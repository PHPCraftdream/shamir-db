use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use shamir_collections::{THasher, TMap};
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::lockout::Subnet;
use shamir_connect::server::rate_limit::{InMemoryRateLimiter, RateDecision, RateLimiter};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span};

#[cfg(test)]
#[path = "tests/maintenance_tests.rs"]
mod tests;

const NOTICE_CAPACITY: usize = 256;
const SUBNET_CAPACITY: usize = 256;
const FAILURE_CAPACITY: usize = 4096;
const FLUSH_LIMIT: usize = 16;
const IDLE_NS: u64 = 300_000_000_000;

/// A credential-safe REST audit notice. IDs are admitted only after verification.
#[derive(Clone, Copy)]
pub(super) enum AuditNotice {
    Failure {
        subnet: Subnet,
        now_ns: u64,
    },
    Exposed {
        id: [u8; 16],
        owner: [u8; 16],
        subnet: Subnet,
        now_ns: u64,
    },
}

struct FailureEntry {
    limiter: Arc<InMemoryRateLimiter>,
    last_seen_ns: AtomicU64,
}

/// Bounded per-subnet failure limiter; existing subnets remain admitted at capacity.
pub(super) struct FailureLimiter {
    entries: DashMap<Subnet, FailureEntry, THasher>,
    count: AtomicUsize,
}

impl FailureLimiter {
    pub(super) fn new(_now_ns: u64) -> Self {
        Self {
            entries: DashMap::with_hasher(THasher::default()),
            count: AtomicUsize::new(0),
        }
    }

    pub(super) fn check(&self, subnet: Subnet, now_ns: u64) -> RateDecision {
        if let Some(entry) = self.entries.get(&subnet) {
            entry.last_seen_ns.fetch_max(now_ns, Ordering::Relaxed);
            return entry.limiter.check(subnet, now_ns);
        }
        if !self.reserve() {
            if let Some(entry) = self.entries.get(&subnet) {
                entry.last_seen_ns.fetch_max(now_ns, Ordering::Relaxed);
                return entry.limiter.check(subnet, now_ns);
            }
            return RateDecision::RateLimited {
                retry_after_secs: 1,
            };
        }
        match self.entries.entry(subnet) {
            Entry::Occupied(entry) => {
                self.count.fetch_sub(1, Ordering::AcqRel);
                entry
                    .get()
                    .last_seen_ns
                    .fetch_max(now_ns, Ordering::Relaxed);
                entry.get().limiter.check(subnet, now_ns)
            }
            Entry::Vacant(entry) => {
                let limiter = Arc::new(InMemoryRateLimiter::new(now_ns));
                let decision = limiter.check(subnet, now_ns);
                entry.insert(FailureEntry {
                    limiter,
                    last_seen_ns: AtomicU64::new(now_ns),
                });
                decision
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

    fn gc(&self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(IDLE_NS);
        let mut removed = 0usize;
        self.entries.retain(|_, entry| {
            let keep = entry.last_seen_ns.load(Ordering::Relaxed) >= cutoff;
            if keep {
                entry.limiter.gc(now_ns);
            } else {
                removed += 1;
            }
            keep
        });
        // Keep removed entries charged until retain has physically removed them.
        self.count.fetch_sub(removed, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(super) fn tracked_subnets(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy)]
struct FailureAggregate {
    count: u64,
    last_ns: u64,
}

#[derive(Clone, Copy)]
enum AuditWrite {
    Failure {
        subnet: Subnet,
        count: u64,
        now_ns: u64,
    },
    Exposed {
        id: [u8; 16],
        owner: [u8; 16],
        subnet: Subnet,
        now_ns: u64,
    },
}

/// REST maintenance worker: bounded audit queue, aggregation and limiter GC.
pub(super) struct RestMaintenance {
    sender: mpsc::Sender<AuditNotice>,
    failures: Arc<FailureLimiter>,
    stop: CancellationToken,
    task: AsyncMutex<Option<JoinHandle<()>>>,
}

impl RestMaintenance {
    pub(super) fn new(
        audit: Arc<AuditChainWriter>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        service_stop: CancellationToken,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(NOTICE_CAPACITY);
        let failures = Arc::new(FailureLimiter::new(clock().unwrap_or_default()));
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let worker_failures = Arc::clone(&failures);
        let task = tokio::spawn(
            run_flusher(
                receiver,
                audit,
                clock,
                worker_failures,
                worker_stop,
                service_stop,
            )
            .in_current_span(),
        );
        Self {
            sender,
            failures,
            stop,
            task: AsyncMutex::new(Some(task)),
        }
    }

    pub(super) fn failures(&self) -> &Arc<FailureLimiter> {
        &self.failures
    }

    pub(super) fn record_failure(&self, subnet: Subnet, now_ns: u64) {
        let _ = self
            .sender
            .try_send(AuditNotice::Failure { subnet, now_ns });
    }

    pub(super) fn record_exposed(
        &self,
        id: [u8; 16],
        owner: [u8; 16],
        subnet: Subnet,
        now_ns: u64,
    ) {
        let _ = self.sender.try_send(AuditNotice::Exposed {
            id,
            owner,
            subnet,
            now_ns,
        });
    }

    pub(super) async fn shutdown(&self) {
        self.stop.cancel();
        let task = { self.task.lock().await.take() };
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
    mut receiver: mpsc::Receiver<AuditNotice>,
    audit: Arc<AuditChainWriter>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    failures: Arc<FailureLimiter>,
    stop: CancellationToken,
    service_stop: CancellationToken,
) {
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
    let mut aggregates = TMap::<Subnet, FailureAggregate>::default();
    let mut exposed = VecDeque::<AuditNotice>::with_capacity(NOTICE_CAPACITY);
    let mut disabled = false;
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = service_stop.cancelled() => break,
            notice = receiver.recv() => {
                let Some(notice) = notice else { break };
                match notice {
                    AuditNotice::Failure { subnet, now_ns } => {
                        if let Some(entry) = aggregates.get_mut(&subnet) {
                            entry.count = entry.count.saturating_add(1);
                            entry.last_ns = entry.last_ns.max(now_ns);
                        } else if aggregates.len() < SUBNET_CAPACITY {
                            aggregates.insert(subnet, FailureAggregate { count: 1, last_ns: now_ns });
                        }
                    }
                    notice @ AuditNotice::Exposed { .. } => {
                        if exposed.len() < NOTICE_CAPACITY { exposed.push_back(notice); }
                    }
                }
            }
            _ = interval.tick() => {
                let mut batch = Vec::with_capacity(FLUSH_LIMIT);
                while batch.len() < FLUSH_LIMIT {
                    let take_failure = !aggregates.is_empty()
                        && (exposed.is_empty() || batch.len() % 2 == 1);
                    if take_failure {
                        let next = aggregates.iter().next().map(|(subnet, _)| *subnet);
                        if let Some(subnet) = next {
                            if let Some(summary) = aggregates.shift_remove(&subnet) {
                                batch.push(AuditWrite::Failure { subnet, count: summary.count, now_ns: summary.last_ns });
                                continue;
                            }
                        }
                    }
                    if let Some(AuditNotice::Exposed { id, owner, subnet, now_ns }) = exposed.pop_front() {
                        batch.push(AuditWrite::Exposed { id, owner, subnet, now_ns });
                        continue;
                    }
                    let next = aggregates.iter().next().map(|(subnet, _)| *subnet);
                    let Some(subnet) = next else { break };
                    if let Some(summary) = aggregates.shift_remove(&subnet) {
                        batch.push(AuditWrite::Failure { subnet, count: summary.count, now_ns: summary.last_ns });
                    }
                }
                if !disabled && !batch.is_empty() {
                    let span = Span::current();
                    let task_audit = Arc::clone(&audit);
                    let handle = tokio::task::spawn_blocking(move || span.in_scope(|| {
                        for write in batch {
                            let result = match write {
                                AuditWrite::Failure { subnet, count, now_ns } => {
                                    let details = match rmp_serde::to_vec_named(&("rest", "invalid", count)) { Ok(value) => value, Err(_) => return false };
                                    task_audit.append("api_token_auth_failed", "token", "", format_subnet(subnet), [0; 8], "invalid", details, now_ns)
                                }
                                AuditWrite::Exposed { id, owner, subnet, now_ns } => {
                                    let details = match rmp_serde::to_vec_named(&("rest", id, owner)) { Ok(value) => value, Err(_) => return false };
                                    task_audit.append("api_token_exposed", "token", "", format_subnet(subnet), [0; 8], "plaintext_denied", details, now_ns)
                                }
                            };
                            if result.is_err() { return false; }
                        }
                        true
                    })).await;
                    if !matches!(handle, Ok(true)) { disabled = true; }
                }
            }
            _ = gc_interval.tick() => {
                if let Some(now_ns) = clock() { failures.gc(now_ns); }
            }
        }
    }
}

fn format_subnet(subnet: Subnet) -> String {
    match subnet {
        Subnet::V4(value) => format!("v4:{value:?}"),
        Subnet::V6(value) => format!("v6:{value:?}"),
    }
}
