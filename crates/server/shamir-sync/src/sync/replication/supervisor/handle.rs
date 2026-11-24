use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shamir_db::repl_binding::SubscriptionLease;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Test seam awaited between admission and spawn to replace the row in that window.
#[cfg(test)]
pub(super) type StartRaceHook = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

/// A running subscription: its stop signal, spawned loop tasks, and the
/// profile + binding generation it was started against (to detect a rebind
/// or a replaced binding on reconcile).
///
/// `lease` is the [`SubscriptionLease`] every loop of this handle runs
/// under; `dones[i]` is loop i's done flag (Release/Acquire), set by the
/// loop's own wrapper task AFTER `run_follower_loop` returned. A handle is
/// STALE when its lease went invalid OR all its loops finished.
/// (round 5, A1). NOT `Clone` — `JoinHandle` is not; the spawned loops get
/// an `Arc` of the shared registry instead.
pub(super) struct SubHandle {
    pub(super) cancel: CancellationToken,
    pub(super) joins: Vec<JoinHandle<()>>,
    /// Per-loop done flags, parallel to `joins` (round 5, A1). One flag per
    /// spawned loop — a shared flag would wrongly mark the whole handle done
    /// when only one of several target loops finished.
    pub(super) dones: Vec<Arc<AtomicBool>>,
    /// Set by the last exiting wrapper after it invalidated the lease and before
    /// it tries to deregister this handle: teardown is complete.
    pub(super) torn_down: Arc<AtomicBool>,
    /// The lease all loops of this handle run under (round 5, A1). Arc
    /// identity of the lease identifies THIS handle: a newer start installs
    /// a handle carrying a different lease.
    pub(super) lease: Arc<SubscriptionLease>,
    pub(super) profile: String,
    pub(super) generation: u64,
    pub(super) targets: Vec<(String, String)>,
}

impl SubHandle {
    /// Round-5 A1: `true` when the handle is stale — its lease went invalid
    /// (superseded by a newer lease / invalidated by the loop's own exit) or
    /// every loop's wrapper task has set its done flag (the loops returned).
    /// A stale handle's rows must be re-started through the normal
    /// start-barrier path instead of being kept as running.
    pub(super) fn stale(&self) -> bool {
        !self.lease.valid.load(Ordering::Acquire)
            || self.dones.iter().all(|d| d.load(Ordering::Acquire))
    }

    /// Cancel the loops and detach the join handles. Cancellation is GRACEFUL
    /// (token only, no `abort()`): each loop observes the token at its next
    /// await and returns, so the wrapper's exit housekeeping (done flag,
    /// lease invalidation, self-deregistration) always completes before the
    /// caller's `join.await` returns.
    pub(super) fn stop(&self) {
        self.cancel.cancel();
    }
}
