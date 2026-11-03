use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use shamir_collections::{THasher, TMap};
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::lockout::Subnet;
use shamir_connect::server::rate_limit::RateDecision;
use shamir_connect::server::token::Endpoint;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;
use tracing::Instrument;

use super::bucket::BucketCache;

#[cfg(test)]
#[path = "tests/maintenance_tests.rs"]
mod tests;

const NOTICE_CAPACITY: usize = 256;
const SUBNET_CAPACITY: usize = 256;
const FAILURE_CAPACITY: usize = 4096;
const FLUSH_LIMIT: usize = 16;
const SAMPLE_CAPACITY: usize = 4;
const WARMUP_NS: u64 = 60_000_000_000;
const IDLE_NS: u64 = 300_000_000_000;
const EVICT_SCAN_INTERVAL_NS: u64 = 100_000_000;
const SHUTDOWN_DRAIN: Duration = Duration::from_secs(2);

/// Credential-safe audit notice; channel is a fixed endpoint enum.
#[derive(Clone, Copy)]
pub(super) enum AuditNotice {
    Failure {
        channel: Endpoint,
        subnet: Subnet,
        now_ns: u64,
    },
    Exposed {
        channel: Endpoint,
        id: [u8; 16],
        owner: [u8; 16],
        subnet: Subnet,
        now_ns: u64,
    },
}

struct FailureEntry {
    window_start_ns: AtomicU64,
    count: AtomicUsize,
    last_seen_ns: AtomicU64,
}

/// Bounded per-subnet failure limiter; idle entries are removed by periodic GC.
pub(super) struct FailureLimiter {
    entries: DashMap<Subnet, FailureEntry, THasher>,
    count: AtomicUsize,
    warmup_anchor_ns: AtomicU64,
    last_evict_scan_ns: AtomicU64,
    #[cfg(test)]
    eviction_scans: AtomicUsize,
}
impl FailureLimiter {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::with_warmup_anchor(None)
    }

    pub(super) fn with_warmup_anchor(anchor: Option<u64>) -> Self {
        Self {
            entries: DashMap::with_hasher(THasher::default()),
            count: AtomicUsize::new(0),
            warmup_anchor_ns: AtomicU64::new(anchor.unwrap_or(u64::MAX)),
            last_evict_scan_ns: AtomicU64::new(u64::MAX),
            #[cfg(test)]
            eviction_scans: AtomicUsize::new(0),
        }
    }
    pub(super) fn check(&self, subnet: Subnet, now_ns: u64) -> RateDecision {
        let mut anchor = self.warmup_anchor_ns.load(Ordering::Acquire);
        if anchor == u64::MAX {
            anchor = match self.warmup_anchor_ns.compare_exchange(
                u64::MAX,
                now_ns,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => now_ns,
                Err(value) => value,
            };
        }
        let limit = if now_ns.saturating_sub(anchor) < WARMUP_NS {
            2
        } else {
            10
        };
        if let Some(mut entry) = self.entries.get_mut(&subnet) {
            return Self::debit(entry.value_mut(), now_ns, limit);
        }

        // Reserve before inserting; concurrent new keys cannot exceed the hard cap.
        let reserved = self.reserve()
            || (self.claim_evict_scan(now_ns) && self.evict_oldest_idle(now_ns) && self.reserve());
        if !reserved {
            return RateDecision::RateLimited {
                retry_after_secs: 1,
            };
        }
        match self.entries.entry(subnet) {
            Entry::Occupied(mut entry) => {
                self.count.fetch_sub(1, Ordering::AcqRel);
                Self::debit(entry.get_mut(), now_ns, limit)
            }
            Entry::Vacant(entry) => {
                let mut inserted = entry.insert(FailureEntry {
                    window_start_ns: AtomicU64::new(now_ns),
                    count: AtomicUsize::new(0),
                    last_seen_ns: AtomicU64::new(now_ns),
                });
                Self::debit(inserted.value_mut(), now_ns, limit)
            }
        }
    }
    fn debit(entry: &mut FailureEntry, now_ns: u64, limit: usize) -> RateDecision {
        entry.last_seen_ns.fetch_max(now_ns, Ordering::Relaxed);
        let start = entry.window_start_ns.load(Ordering::Relaxed);
        if now_ns.saturating_sub(start) >= 1_000_000_000 {
            entry.window_start_ns.store(now_ns, Ordering::Relaxed);
            entry.count.store(0, Ordering::Relaxed);
        }
        let used = entry.count.load(Ordering::Relaxed);
        if used < limit {
            entry.count.store(used + 1, Ordering::Relaxed);
            RateDecision::Allowed
        } else {
            RateDecision::RateLimited {
                retry_after_secs: 1,
            }
        }
    }
    fn reserve(&self) -> bool {
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count
                    .checked_add(1)
                    .filter(|next| *next <= FAILURE_CAPACITY)
            })
            .is_ok()
    }
    fn claim_evict_scan(&self, now_ns: u64) -> bool {
        let previous = self.last_evict_scan_ns.load(Ordering::Acquire);
        if previous != u64::MAX && now_ns >= previous && now_ns - previous < EVICT_SCAN_INTERVAL_NS
        {
            return false;
        }
        let claimed = self
            .last_evict_scan_ns
            .compare_exchange(previous, now_ns, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        #[cfg(test)]
        if claimed {
            self.eviction_scans.fetch_add(1, Ordering::Relaxed);
        }
        claimed
    }

    fn evict_oldest_idle(&self, now_ns: u64) -> bool {
        let cutoff = now_ns.saturating_sub(1_000_000_000);
        let candidate = self
            .entries
            .iter()
            .filter_map(|entry| {
                let seen = entry.last_seen_ns.load(Ordering::Relaxed);
                (seen < cutoff).then_some((*entry.key(), seen))
            })
            .min_by_key(|(_, seen)| *seen);
        if let Some((subnet, seen)) = candidate {
            if self
                .entries
                .remove_if(&subnet, |_, entry| {
                    entry.last_seen_ns.load(Ordering::Relaxed) == seen
                })
                .is_some()
            {
                self.count.fetch_sub(1, Ordering::AcqRel);
                return true;
            }
        }
        false
    }
    fn gc(&self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(IDLE_NS);
        let mut removed = 0usize;
        self.entries.retain(|_, entry| {
            let keep = entry.last_seen_ns.load(Ordering::Relaxed) >= cutoff;
            if !keep {
                removed += 1;
            }
            keep
        });
        self.count.fetch_sub(removed, Ordering::AcqRel);
    }
    #[cfg(test)]
    pub(super) fn tracked_subnets(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(super) fn eviction_scans(&self) -> usize {
        self.eviction_scans.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy)]
struct FailureAggregate {
    count: u64,
    last_ns: u64,
}
/// Summary tuple: channel, result, total, tracked distinct, subnet samples, dropped, untracked, exposure-dropped.
struct FailureSummary {
    channel: Endpoint,
    total: u64,
    distinct_tracked: usize,
    samples: [Option<Subnet>; SAMPLE_CAPACITY],
    sample_count: usize,
    untracked: u64,
    dropped: u64,
    exposure_dropped: u64,
    last_ns: u64,
}
impl FailureSummary {
    fn new(channel: Endpoint) -> Self {
        Self {
            channel,
            total: 0,
            distinct_tracked: 0,
            samples: [None; SAMPLE_CAPACITY],
            sample_count: 0,
            untracked: 0,
            dropped: 0,
            exposure_dropped: 0,
            last_ns: 0,
        }
    }
    fn record(
        &mut self,
        aggregates: &mut TMap<Subnet, FailureAggregate>,
        subnet: Subnet,
        now_ns: u64,
    ) {
        self.total = self.total.saturating_add(1);
        self.last_ns = self.last_ns.max(now_ns);
        if let Some(entry) = aggregates.get_mut(&subnet) {
            entry.count = entry.count.saturating_add(1);
            entry.last_ns = entry.last_ns.max(now_ns);
        } else if aggregates.len() < SUBNET_CAPACITY {
            aggregates.insert(
                subnet,
                FailureAggregate {
                    count: 1,
                    last_ns: now_ns,
                },
            );
            self.distinct_tracked = self.distinct_tracked.saturating_add(1);
            if self.sample_count < SAMPLE_CAPACITY {
                self.samples[self.sample_count] = Some(subnet);
                self.sample_count += 1;
            }
        } else {
            self.untracked = self.untracked.saturating_add(1);
        }
    }
    fn is_empty(&self) -> bool {
        self.total == 0 && self.dropped == 0 && self.exposure_dropped == 0
    }
    fn details(&self) -> Result<Vec<u8>, ()> {
        rmp_serde::to_vec_named(&(
            channel_label(self.channel),
            "invalid",
            self.total,
            self.distinct_tracked,
            &self.samples,
            self.dropped,
            self.untracked,
            self.exposure_dropped,
        ))
        .map_err(|_| ())
    }
}
struct AuditWrite {
    event: &'static str,
    outcome: &'static str,
    resource: String,
    details: Vec<u8>,
    now_ns: u64,
    exposure_id: Option<(Endpoint, [u8; 16])>,
}
struct FlusherReceivers(mpsc::Receiver<AuditNotice>, mpsc::Receiver<AuditNotice>);
struct DropCounters(
    Arc<AtomicU64>,
    Arc<AtomicU64>,
    Arc<AtomicU64>,
    Arc<AtomicU64>,
    Arc<AtomicBool>,
);
struct SweepTargets(Arc<FailureLimiter>, Arc<BucketCache>);

/// REST maintenance worker with independent bounded notice channels.
pub(super) struct RestMaintenance {
    sender: mpsc::Sender<AuditNotice>,
    exposure_sender: mpsc::Sender<AuditNotice>,
    failure_drops: Arc<AtomicU64>,
    ws_failure_drops: Arc<AtomicU64>,
    exposure_drops: Arc<AtomicU64>,
    ws_exposure_drops: Arc<AtomicU64>,
    disabled: Arc<AtomicBool>,
    failures: Arc<FailureLimiter>,
    stop: CancellationToken,
    task: AsyncMutex<Option<JoinHandle<()>>>,
}
impl RestMaintenance {
    pub(super) fn new(
        audit: Arc<AuditChainWriter>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        service_stop: CancellationToken,
        buckets: Arc<BucketCache>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(NOTICE_CAPACITY);
        let (exposure_sender, exposure_receiver) = mpsc::channel(NOTICE_CAPACITY);
        let failure_drops = Arc::new(AtomicU64::new(0));
        let ws_failure_drops = Arc::new(AtomicU64::new(0));
        let exposure_drops = Arc::new(AtomicU64::new(0));
        let ws_exposure_drops = Arc::new(AtomicU64::new(0));
        let disabled = Arc::new(AtomicBool::new(false));
        let failures = Arc::new(FailureLimiter::with_warmup_anchor(clock()));
        let stop = CancellationToken::new();
        let task = tokio::spawn(
            run_flusher(
                FlusherReceivers(receiver, exposure_receiver),
                audit,
                clock,
                SweepTargets(Arc::clone(&failures), buckets),
                DropCounters(
                    Arc::clone(&failure_drops),
                    Arc::clone(&ws_failure_drops),
                    Arc::clone(&exposure_drops),
                    Arc::clone(&ws_exposure_drops),
                    Arc::clone(&disabled),
                ),
                stop.clone(),
                service_stop,
            )
            .in_current_span()
            .with_current_subscriber(),
        );
        Self {
            sender,
            exposure_sender,
            failure_drops,
            ws_failure_drops,
            exposure_drops,
            ws_exposure_drops,
            disabled: Arc::clone(&disabled),
            failures,
            stop,
            task: AsyncMutex::new(Some(task)),
        }
    }
    pub(super) fn failures(&self) -> &Arc<FailureLimiter> {
        &self.failures
    }
    pub(super) fn record_failure(&self, channel: Endpoint, subnet: Subnet, now_ns: u64) {
        if self
            .sender
            .try_send(AuditNotice::Failure {
                channel,
                subnet,
                now_ns,
            })
            .is_err()
        {
            let counter = match channel {
                Endpoint::Rest => &self.failure_drops,
                Endpoint::Ws => &self.ws_failure_drops,
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(super) fn record_exposed(
        &self,
        channel: Endpoint,
        id: [u8; 16],
        owner: [u8; 16],
        subnet: Subnet,
        now_ns: u64,
    ) {
        let drop_counter = match channel {
            Endpoint::Rest => &self.exposure_drops,
            Endpoint::Ws => &self.ws_exposure_drops,
        };
        if self.disabled.load(Ordering::Acquire) {
            drop_counter.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(token_id = ?id, "REST token exposure audit unavailable");
            return;
        }
        if self
            .exposure_sender
            .try_send(AuditNotice::Exposed {
                channel,
                id,
                owner,
                subnet,
                now_ns,
            })
            .is_err()
        {
            drop_counter.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(token_id = ?id, "REST token exposure audit channel full or closed");
        }
    }
    pub(super) async fn shutdown(&self) {
        self.stop.cancel();
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}
impl Drop for RestMaintenance {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn run_flusher(
    receivers: FlusherReceivers,
    audit: Arc<AuditChainWriter>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    targets: SweepTargets,
    drops: DropCounters,
    stop: CancellationToken,
    service_stop: CancellationToken,
) {
    let FlusherReceivers(mut receiver, mut exposure_receiver) = receivers;
    let SweepTargets(failures, buckets) = targets;
    let DropCounters(
        failure_drops,
        ws_failure_drops,
        exposure_drops,
        ws_exposure_drops,
        disabled_shared,
    ) = drops;
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut gc_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    gc_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut summaries = [
        FailureSummary::new(Endpoint::Rest),
        FailureSummary::new(Endpoint::Ws),
    ];
    let mut aggregates = [
        TMap::<Subnet, FailureAggregate>::default(),
        TMap::<Subnet, FailureAggregate>::default(),
    ];
    let mut exposed = VecDeque::<AuditNotice>::with_capacity(NOTICE_CAPACITY);
    let mut exposure_overflow = [0u64; 2];
    let mut writer: Option<JoinHandle<bool>> = None;
    let mut disabled = false;
    let mut failure_closed = false;
    let mut exposure_closed = false;
    let mut stopping = false;
    // At the two-second shutdown limit, detach an in-flight blocking batch rather than delay shutdown.
    let deadline = tokio::time::sleep(Duration::from_secs(365 * 24 * 60 * 60));
    tokio::pin!(deadline);
    let mut flush_requested = false;
    let mut final_drops_taken = false;

    loop {
        tokio::select! {
            _ = stop.cancelled(), if !stopping => {
                stopping = true;
                receiver.close();
                exposure_receiver.close();
                deadline.as_mut().reset(tokio::time::Instant::now() + SHUTDOWN_DRAIN);
            }
            _ = service_stop.cancelled(), if !stopping => {
                stopping = true;
                receiver.close();
                exposure_receiver.close();
                deadline.as_mut().reset(tokio::time::Instant::now() + SHUTDOWN_DRAIN);
            }
            _ = &mut deadline, if stopping => {
                if writer.is_some()
                    || summaries.iter().any(|summary| !summary.is_empty())
                    || !exposed.is_empty()
                {
                    let summary_total = summaries[0].total.saturating_add(summaries[1].total);
                    tracing::warn!(
                        summary_total,
                        pending_exposures = exposed.len(),
                        failure_dropped = summaries[0].dropped.saturating_add(summaries[1].dropped),
                        exposure_dropped = summaries[0].exposure_dropped.saturating_add(summaries[1].exposure_dropped),
                        "REST token audit shutdown drain deadline expired"
                    );
                }
                drop(writer.take());
                break;
            }
            notice = receiver.recv(), if !failure_closed => match notice {
                Some(AuditNotice::Failure { channel, subnet, now_ns }) => {
                    let index = endpoint_index(channel);
                    summaries[index].record(&mut aggregates[index], subnet, now_ns);
                }
                Some(AuditNotice::Exposed { channel, id, .. }) => {
                    let index = endpoint_index(channel);
                    exposure_overflow[index] = exposure_overflow[index].saturating_add(1);
                    tracing::warn!(token_id = ?id, "REST token exposure audit arrived on failure channel");
                }
                None => {
                    failure_closed = true;
                },
            },
            notice = exposure_receiver.recv(), if !exposure_closed => match notice {
                Some(notice @ AuditNotice::Exposed { .. })
                    if exposed.len() < NOTICE_CAPACITY && !disabled =>
                {
                    exposed.push_back(notice);
                }
                Some(AuditNotice::Exposed { channel, id, .. }) => {
                    let index = endpoint_index(channel);
                    exposure_overflow[index] = exposure_overflow[index].saturating_add(1);
                    tracing::warn!(token_id = ?id, "REST token exposure audit not persisted");
                }
                Some(AuditNotice::Failure { .. }) => {},
                None => {
                    exposure_closed = true;
                },
            },
            result = async { writer.as_mut().expect("writer exists").await }, if writer.is_some() => {
                writer = None;
                if !matches!(result, Ok(true)) {
                    disabled = true;
                    disabled_shared.store(true, Ordering::Release);
                    tracing::warn!("REST token audit persistence disabled");
                    for notice in exposed.drain(..) {
                        if let AuditNotice::Exposed { channel, id, .. } = notice {
                            let index = endpoint_index(channel);
                            exposure_overflow[index] = exposure_overflow[index].saturating_add(1);
                            tracing::warn!(token_id = ?id, "REST token exposure audit not persisted");
                        }
                    }
                }
                if stopping {
                    flush_requested = true;
                }
            }
            _ = interval.tick(), if writer.is_none() && !disabled => {
                flush_requested = true;
            }
            _ = gc_interval.tick() => {
                if let Some(now_ns) = clock() {
                    failures.gc(now_ns);
                    buckets.sweep(now_ns);
                }
            }
        }
        if disabled_shared.load(Ordering::Acquire) {
            disabled = true;
        }
        if stopping && failure_closed && exposure_closed && !final_drops_taken {
            let rest_dropped = failure_drops.swap(0, Ordering::AcqRel);
            let ws_dropped = ws_failure_drops.swap(0, Ordering::AcqRel);
            summaries[0].dropped = summaries[0].dropped.saturating_add(rest_dropped);
            summaries[1].dropped = summaries[1].dropped.saturating_add(ws_dropped);
            summaries[0].exposure_dropped = summaries[0]
                .exposure_dropped
                .saturating_add(exposure_drops.swap(0, Ordering::AcqRel))
                .saturating_add(std::mem::take(&mut exposure_overflow[0]));
            summaries[1].exposure_dropped = summaries[1]
                .exposure_dropped
                .saturating_add(ws_exposure_drops.swap(0, Ordering::AcqRel))
                .saturating_add(std::mem::take(&mut exposure_overflow[1]));
            final_drops_taken = true;
            flush_requested =
                summaries.iter().any(|summary| !summary.is_empty()) || !exposed.is_empty();
        }
        if stopping && failure_closed && exposure_closed && writer.is_none() && disabled {
            break;
        }
        if !final_drops_taken {
            let rest_dropped = failure_drops.swap(0, Ordering::AcqRel);
            let ws_dropped = ws_failure_drops.swap(0, Ordering::AcqRel);
            summaries[0].dropped = summaries[0].dropped.saturating_add(rest_dropped);
            summaries[1].dropped = summaries[1].dropped.saturating_add(ws_dropped);
            summaries[0].exposure_dropped = summaries[0]
                .exposure_dropped
                .saturating_add(exposure_drops.swap(0, Ordering::AcqRel))
                .saturating_add(std::mem::take(&mut exposure_overflow[0]));
            summaries[1].exposure_dropped = summaries[1]
                .exposure_dropped
                .saturating_add(ws_exposure_drops.swap(0, Ordering::AcqRel))
                .saturating_add(std::mem::take(&mut exposure_overflow[1]));
        }
        if stopping
            && failure_closed
            && exposure_closed
            && writer.is_none()
            && summaries.iter().all(FailureSummary::is_empty)
            && exposed.is_empty()
        {
            break;
        }
        if stopping
            && failure_closed
            && exposure_closed
            && writer.is_none()
            && (summaries.iter().any(|summary| !summary.is_empty()) || !exposed.is_empty())
        {
            flush_requested = true;
        }
        let should_flush = writer.is_none()
            && !disabled
            && flush_requested
            && (!stopping || (failure_closed && exposure_closed));
        if should_flush {
            flush_requested = false;
            let mut batch = Vec::with_capacity(FLUSH_LIMIT);
            for (index, summary) in summaries.iter_mut().enumerate() {
                if batch.len() >= FLUSH_LIMIT {
                    break;
                }
                if !summary.is_empty() {
                    let now_ns = if summary.last_ns == 0 {
                        clock().unwrap_or_default()
                    } else {
                        summary.last_ns
                    };
                    match summary.details() {
                        Ok(details) => {
                            batch.push(AuditWrite {
                                event: "api_token_auth_failures",
                                outcome: "invalid",
                                resource: String::new(),
                                details,
                                now_ns,
                                exposure_id: None,
                            });
                            *summary = FailureSummary::new(if index == 0 {
                                Endpoint::Rest
                            } else {
                                Endpoint::Ws
                            });
                            aggregates[index].clear();
                        }
                        Err(()) => {
                            disabled = true;
                            disabled_shared.store(true, Ordering::Release);
                            tracing::warn!("REST token audit serialization disabled");
                        }
                    }
                }
            }
            while batch.len() < FLUSH_LIMIT {
                match exposed.pop_front() {
                    Some(AuditNotice::Exposed {
                        channel,
                        id,
                        owner,
                        subnet,
                        now_ns,
                    }) => {
                        let index = endpoint_index(channel);
                        if let Some(details) = serialize_exposure(
                            channel,
                            id,
                            &mut summaries[index].exposure_dropped,
                            || {
                                rmp_serde::to_vec_named(&(channel_label(channel), id, owner))
                                    .map_err(|_| ())
                            },
                        ) {
                            batch.push(AuditWrite {
                                event: "api_token_exposed",
                                outcome: "plaintext_denied",
                                resource: format_subnet(subnet),
                                details,
                                now_ns,
                                exposure_id: Some((channel, id)),
                            });
                        }
                    }
                    Some(AuditNotice::Failure { .. }) => {}
                    None => break,
                }
            }
            if !batch.is_empty() && !disabled {
                let task_audit = Arc::clone(&audit);
                let task_exposure_drops = Arc::clone(&exposure_drops);
                let task_ws_exposure_drops = Arc::clone(&ws_exposure_drops);
                writer = Some(tokio::task::spawn_blocking(move || {
                    let mut writes = batch.into_iter();
                    while let Some(write) = writes.next() {
                        if task_audit
                            .append(
                                write.event,
                                "token",
                                "",
                                write.resource,
                                [0; 8],
                                write.outcome,
                                write.details,
                                write.now_ns,
                            )
                            .is_err()
                        {
                            if let Some((channel, id)) = write.exposure_id {
                                let counter = match channel {
                                    Endpoint::Rest => &task_exposure_drops,
                                    Endpoint::Ws => &task_ws_exposure_drops,
                                };
                                counter.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!(token_id = ?id, "REST token exposure audit append failed");
                            }
                            for pending in writes {
                                if let Some((channel, id)) = pending.exposure_id {
                                    let counter = match channel {
                                        Endpoint::Rest => &task_exposure_drops,
                                        Endpoint::Ws => &task_ws_exposure_drops,
                                    };
                                    counter.fetch_add(1, Ordering::Relaxed);
                                    tracing::warn!(
                                        token_id = ?id,
                                        "REST token exposure audit not persisted"
                                    );
                                }
                            }
                            return false;
                        }
                    }
                    true
                }));
            }
        }
    }
}
fn serialize_exposure<F>(
    channel: Endpoint,
    id: [u8; 16],
    drops: &mut u64,
    serialize: F,
) -> Option<Vec<u8>>
where
    F: FnOnce() -> Result<Vec<u8>, ()>,
{
    match serialize() {
        Ok(details) => Some(details),
        Err(()) => {
            *drops = drops.saturating_add(1);
            tracing::warn!(token_id = ?id, channel = channel_label(channel), "REST token exposure audit serialization failed");
            None
        }
    }
}

fn endpoint_index(endpoint: Endpoint) -> usize {
    match endpoint {
        Endpoint::Rest => 0,
        Endpoint::Ws => 1,
    }
}
fn channel_label(endpoint: Endpoint) -> &'static str {
    match endpoint {
        Endpoint::Rest => "rest",
        Endpoint::Ws => "ws",
    }
}
fn format_subnet(subnet: Subnet) -> String {
    match subnet {
        Subnet::V4(value) => format!("v4:{value:?}"),
        Subnet::V6(value) => format!("v6:{value:?}"),
    }
}
