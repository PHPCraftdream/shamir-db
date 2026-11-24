//! Durable snapshot sinks that bridge the in-memory lockout and rate-limit
//! stores to the fjall-backed `ServerMetaStore`.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::lockout::{
    InMemoryLockoutStore, LockoutSnapshot, LockoutSnapshotError, LockoutSnapshotSink,
};
use shamir_connect::server::rate_limit::{
    InMemoryRateLimiter, RateLimitSnapshot, RateLimitSnapshotError, RateLimitSnapshotSink,
};
use tokio_util::sync::CancellationToken;

use crate::server::server_handle::MetaSnapshotTask;
use crate::server_meta::ServerMetaStore;

/// Default interval between lockout-snapshot writes. 60s matches the
/// audit-checkpoint cadence and balances disk pressure (one fjall store write
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
/// Shutdown is driven by the shared root `shutdown` [`CancellationToken`]:
/// `ServerHandle::shutdown` cancels it once and this task observes the cancel
/// on its next `select!` poll. The task writes one final snapshot of each on
/// the way out so a clean restart sees the freshest possible state —
/// important for both the durability story and for integration tests that
/// boot, do work, shut down, and reboot from the same data dir (the fjall
/// lock on the `server_meta` store cannot be re-acquired until this task drops its
/// `Arc<ServerMetaStore>`).
///
/// CancellationToken (not `Notify`) closes the lossy subscribe-window race:
/// a cancel that lands while this task executes its tick-branch body (instead
/// of parked in `select!`) is still observed on the next loop iteration,
/// because the token's cancelled state is persistent — it does not evaporate.
pub(super) fn spawn_meta_snapshot_task(
    lockout: Arc<InMemoryLockoutStore>,
    rate_limit: Arc<InMemoryRateLimiter>,
    shutdown: CancellationToken,
) -> MetaSnapshotTask {
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(LOCKOUT_SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Discard the immediate first tick — no point in writing an
        // empty snapshot the moment the server boots.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    tracing::debug!("meta_snapshot: shutdown cancelled, writing final snapshots");
                    // OFF the runtime worker: a final `persist_snapshot` is a
                    // synchronous store write (and may `SyncAll`), so writing it
                    // inline here could starve a shutdown deadline timer on a
                    // current-thread runtime (finding: synchronous `server_meta`
                    // write on the teardown path). Graceful shutdown and the failed
                    // launch rollback join the handle under a bound and detach it
                    // only when that bound expires.
                    let lockout = Arc::clone(&lockout);
                    let rate_limit = Arc::clone(&rate_limit);
                    let written = tokio::task::spawn_blocking(move || {
                        let mut failures = Vec::new();
                        if let Err(error) = lockout.persist_snapshot() {
                            failures.push(("final lockout snapshot persist failed", error.to_string()));
                        }
                        if let Err(error) = rate_limit.persist_snapshot() {
                            failures.push(("final rate-limit snapshot persist failed", error.to_string()));
                        }
                        failures
                    })
                    .await;
                    match written {
                        Ok(failures) => {
                            for (what, error) in failures {
                                tracing::warn!(what, %error, "final metadata snapshot persist failed");
                            }
                        }
                        Err(join_error) => tracing::warn!(
                            %join_error,
                            "final metadata snapshot task did not complete"
                        ),
                    }
                    break;
                }
                _ = interval.tick() => {
                    // OFF the runtime worker, exactly like the final write: the
                    // periodic `persist_snapshot` is a synchronous store write
                    // (and may `SyncAll`), so a shutdown beginning during it
                    // must not share a worker with it on a current-thread
                    // runtime. The await keeps the ordering of successive ticks.
                    let lockout = Arc::clone(&lockout);
                    let rate_limit = Arc::clone(&rate_limit);
                    let written = tokio::task::spawn_blocking(move || {
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
                    })
                    .await;
                    if let Err(join_error) = written {
                        tracing::warn!(
                            %join_error,
                            "periodic metadata snapshot task did not complete"
                        );
                    }
                }
            }
        }
    });
    MetaSnapshotTask { handle }
}
