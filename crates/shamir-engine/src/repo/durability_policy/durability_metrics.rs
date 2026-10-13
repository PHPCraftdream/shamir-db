//! Repository-owned telemetry, shared by all handles. Never updated on commit.
//! `shamir_wal_synced_lag_versions` is W - R; `shamir_wal_sync_now_calls`
//! comes from this repository's WAL coordinator; `shamir_replication_pull_held_back_ns`
//! and `_count` measure pull waits, including cancelled waits. Export labels: db/repo only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use shamir_query_types::{ReplicationServe, WalCommitTier};
use shamir_wal::WalGroupCommit;

use super::DurabilityPolicy;

#[derive(Default)]
struct PullSample {
    confirmed: u64,
    served: u64,
}

struct PendingSample {
    first: u64,
    since: Instant,
    observed: Instant,
}

/// One value per repository, not a process-wide last-observed sample.
pub struct DurabilityMetrics {
    pull: ArcSwap<PullSample>,
    pending: ArcSwap<PendingSample>,
    held_ns: AtomicU64,
    held_count: AtomicU64,
}

/// Off-path observation. Pending count is the uncovered version span W - R,
/// not a cardinality of sparse coverage entries. Age is a lower bound since
/// first observation of R + 1, not commit allocation time. Independent WAL
/// counters/watermarks are diagnostic samples, not a coverage certificate.
#[derive(Default)]
pub struct DurabilityMetricsSnapshot {
    pub gauges: [u64; 11],
    pub counters: [u64; 7],
}

impl Default for DurabilityMetrics {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            pull: ArcSwap::from_pointee(PullSample::default()),
            pending: ArcSwap::from_pointee(PendingSample {
                first: 0,
                since: now,
                observed: now,
            }),
            held_ns: AtomicU64::new(0),
            held_count: AtomicU64::new(0),
        }
    }
}

impl DurabilityMetrics {
    pub(crate) fn observe_bound(&self, confirmed: u64, served: u64) {
        self.pull.store(Arc::new(PullSample { confirmed, served }));
    }

    pub(crate) fn hold_pull(&self) -> PullWait<'_> {
        PullWait {
            metrics: self,
            started: Instant::now(),
        }
    }

    /// Scrape only: samples initialized WAL without creating stores or workers.
    pub fn snapshot(
        &self,
        policy: &DurabilityPolicy,
        group: Option<&WalGroupCommit>,
    ) -> DurabilityMetricsSnapshot {
        let pull = self.pull.load();
        let (w, r) = group.map_or((0, 0), |g| {
            (g.max_committed_version(), g.recoverable_through())
        });
        let span = w.saturating_sub(r);
        let first = if span > 0 { r.saturating_add(1) } else { 0 };
        let now = Instant::now();
        self.pending.rcu(|old| {
            // A delayed scrape must not replace a newer observation, reset
            // its age or make the first uncovered position regress.
            if old.observed > now {
                return Arc::clone(old);
            }
            Arc::new(PendingSample {
                first,
                since: if old.first == first { old.since } else { now },
                observed: now,
            })
        });
        let pending = self.pending.load();
        let age = if first == 0 || pending.first != first {
            0
        } else {
            u64::try_from(now.saturating_duration_since(pending.since).as_nanos())
                .unwrap_or(u64::MAX)
        };
        let (calls, rounds, coalesced, errors, duration) = group.map_or((0, 0, 0, 0, 0), |g| {
            (
                g.instance_sync_calls(),
                g.instance_sync_rounds(),
                g.instance_sync_coalesced(),
                g.instance_sync_errors(),
                g.instance_sync_duration_ns(),
            )
        });
        DurabilityMetricsSnapshot {
            gauges: [
                u64::from(policy.wal_commit() == WalCommitTier::Synced),
                u64::from(policy.replication_serve() == ReplicationServe::CompletedReseed),
                u64::from(group.is_some_and(WalGroupCommit::is_mem_sink)),
                span,
                w,
                r,
                pull.confirmed,
                pull.served,
                span,
                first,
                age,
            ],
            counters: [
                calls,
                rounds,
                coalesced,
                errors,
                duration,
                self.held_ns.load(Ordering::Relaxed),
                self.held_count.load(Ordering::Relaxed),
            ],
        }
    }
}

/// Records elapsed time even when the caller drops the pull future.
pub(crate) struct PullWait<'a> {
    metrics: &'a DurabilityMetrics,
    started: Instant,
}

impl Drop for PullWait<'_> {
    fn drop(&mut self) {
        let ns = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let _ = self
            .metrics
            .held_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(old.saturating_add(ns))
            });
        let _ = self
            .metrics
            .held_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(old.saturating_add(1))
            });
    }
}
