//! Durable snapshot sinks that bridge the in-memory lockout and rate-limit
//! stores to the `ServerMetaStore` redb backend.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::lockout::{
    InMemoryLockoutStore, LockoutSnapshot, LockoutSnapshotError, LockoutSnapshotSink,
};
use shamir_connect::server::rate_limit::{
    InMemoryRateLimiter, RateLimitSnapshot, RateLimitSnapshotError, RateLimitSnapshotSink,
};
use tokio::sync::Notify;

use crate::server::server_handle::MetaSnapshotTask;
use crate::server_meta::ServerMetaStore;

/// Default interval between lockout-snapshot writes. 60s matches the
/// audit-checkpoint cadence and balances disk pressure (one redb write
/// per minute) against the loss window for failed-auth bookkeeping
/// (worst-case ~60s of new failures lost on hard crash). The spec IMPL
/// §1.3 ceiling is 5s but is not normative for failed-auth state —
/// the lockout subsystem just needs durability across CLEAN restart, and
/// the rate-limiter's warmup window (spec §8.6) already covers any
/// drift during the recovery window.
const LOCKOUT_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

/// [`LockoutSnapshotSink`] adapter that writes through to the durable
/// `ServerMetaStore`. The store handles its own write transaction +
/// `Durability::Immediate` fsync, so this sink is just a thin glue
/// translator between the trait error type and `MetaError::to_string()`.
pub(super) struct MetaLockoutSink {
    meta: Arc<ServerMetaStore>,
}

impl MetaLockoutSink {
    pub(super) fn new(meta: Arc<ServerMetaStore>) -> Self {
        Self { meta }
    }
}

impl LockoutSnapshotSink for MetaLockoutSink {
    fn save(&self, snapshot: &LockoutSnapshot) -> Result<(), LockoutSnapshotError> {
        self.meta
            .store_lockout_snapshot(snapshot)
            .map_err(|e| LockoutSnapshotError::Storage(e.to_string()))
    }

    fn load(&self) -> Result<Option<LockoutSnapshot>, LockoutSnapshotError> {
        self.meta
            .lockout_snapshot()
            .map_err(|e| LockoutSnapshotError::Storage(e.to_string()))
    }
}

/// [`RateLimitSnapshotSink`] adapter that writes through to the durable
/// `ServerMetaStore` — the rate-limiter analogue of [`MetaLockoutSink`].
pub(super) struct MetaRateLimitSink {
    meta: Arc<ServerMetaStore>,
}

impl MetaRateLimitSink {
    pub(super) fn new(meta: Arc<ServerMetaStore>) -> Self {
        Self { meta }
    }
}

impl RateLimitSnapshotSink for MetaRateLimitSink {
    fn save(&self, snapshot: &RateLimitSnapshot) -> Result<(), RateLimitSnapshotError> {
        self.meta
            .store_ratelimit_snapshot(snapshot)
            .map_err(|e| RateLimitSnapshotError::Storage(e.to_string()))
    }

    fn load(&self) -> Result<Option<RateLimitSnapshot>, RateLimitSnapshotError> {
        self.meta
            .ratelimit_snapshot()
            .map_err(|e| RateLimitSnapshotError::Storage(e.to_string()))
    }
}

/// Background task: every [`LOCKOUT_SNAPSHOT_INTERVAL`], capture the
/// current in-memory lockout state AND rate-limiter buckets and push them
/// through their installed sinks. One tick persists both — no second
/// timer, single shutdown drain. Persist failures are logged at `warn` and
/// the loop continues — losing one snapshot is recoverable on the next
/// tick.
///
/// Shutdown is driven by the returned `stop` `Notify`. The task writes one
/// final snapshot of each on the way out so a clean restart sees the
/// freshest possible state — important for both the durability story and
/// for integration tests that boot, do work, shut down, and reboot from
/// the same data dir (the redb file lock on `server_meta.redb` cannot be
/// re-acquired until this task drops its `Arc<ServerMetaStore>`).
pub(super) fn spawn_meta_snapshot_task(
    lockout: Arc<InMemoryLockoutStore>,
    rate_limit: Arc<InMemoryRateLimiter>,
) -> MetaSnapshotTask {
    let stop = Arc::new(Notify::new());
    let stop_inner = stop.clone();
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(LOCKOUT_SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Discard the immediate first tick — no point in writing an
        // empty snapshot the moment the server boots.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = stop_inner.notified() => {
                    tracing::debug!("meta_snapshot: shutdown notified, writing final snapshots");
                    if let Err(e) = lockout.persist_snapshot() {
                        tracing::warn!(error = %e, "final lockout snapshot persist failed");
                    }
                    if let Err(e) = rate_limit.persist_snapshot() {
                        tracing::warn!(error = %e, "final rate-limit snapshot persist failed");
                    }
                    break;
                }
                _ = interval.tick() => {
                    // `persist_snapshot` is synchronous (sync redb write
                    // under the hood). It's short — measured << 1ms for
                    // typical state sizes — so running it on the runtime
                    // thread is fine. If it ever grows we can wrap in
                    // `spawn_blocking`.
                    match lockout.persist_snapshot() {
                        Ok(true) => {
                            tracing::trace!(
                                failures = lockout.failure_pair_count(),
                                lockouts = lockout.active_lockout_count(),
                                "lockout snapshot persisted",
                            );
                        }
                        Ok(false) => {
                            // No sink installed — should not happen in
                            // production paths since `launch()` always
                            // wires one.
                            tracing::trace!("meta snapshot task: no lockout sink installed");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "lockout snapshot persist failed");
                        }
                    }
                    match rate_limit.persist_snapshot() {
                        Ok(true) => {
                            tracing::trace!(
                                subnets = rate_limit.tracked_subnets(),
                                "rate-limit snapshot persisted",
                            );
                        }
                        Ok(false) => {
                            tracing::trace!("meta snapshot task: no rate-limit sink installed");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "rate-limit snapshot persist failed");
                        }
                    }
                }
            }
        }
    });
    MetaSnapshotTask { handle, stop }
}
