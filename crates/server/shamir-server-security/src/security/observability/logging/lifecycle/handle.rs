//! Handle lifecycle for the observability server: the handle returned by
//! `spawn`, the cancellation token it owns, and the per-side shutdown outcome.
//!
//! Split out of `observability/mod.rs` to keep that file inside the layout
//! limit. Nothing moves in the public API: `observability/mod.rs` re-exports
//! every name below, so `crate::observability::{ObservabilityHandle, ...}`
//! still resolves exactly as before.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::super::ObservabilityState;

/// Per-side bound on joining one observability task in
/// [`ObservabilityHandle::shutdown`].
///
/// Each side gets its OWN bound: the listener may be draining a scrape while
/// the poller is already parked on its token, so a single budget shared by
/// both — or one unbounded sequential await — let a stuck side hide the
/// healthy one. Two sides at this bound stay inside `server_handle.rs`'s
/// `OBS_SHUTDOWN_TIMEOUT` (5 s), so the caller receives a per-side outcome
/// instead of an abandoned future.
pub const OBS_TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Which observability tasks [`ObservabilityHandle::shutdown`] managed to join.
///
/// `true` means that task returned, so it no longer holds the router or its
/// `Arc<ShamirDb>`. `false` means the side missed [`OBS_TASK_JOIN_TIMEOUT`]
/// and is **still draining and may still hold the database** after
/// `shutdown()` returned: the caller must treat the stop as unclean and name
/// exactly that side — a listener finishing a scrape is a different report
/// line from a poller still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservabilityShutdownOutcome {
    /// `axum::serve` returned: no connection is being served any more.
    pub listener_joined: bool,
    /// The process-metrics poller returned: no poll tick is in flight.
    pub poller_joined: bool,
}

/// Handle to a running observability server.
///
/// [`Self::shutdown`] stops it AND waits for both tasks. Dropping the handle
/// cancels the same token (via the private `shutdown_on_drop` field), so a
/// shutdown future abandoned by its deadline cannot leave the poller and the
/// router alive holding the database; the two `JoinHandle`s are still not
/// awaited by the drop, so a dropped handle does not block — the tasks stop on
/// their own at their next cancellation point.
///
/// The cancel-on-drop lives in a PRIVATE FIELD, not in an `impl Drop for
/// ObservabilityHandle`: a type-level `Drop` would make this public type
/// non-move-partial and stop a caller from moving the public `state` field out
/// of it.
pub struct ObservabilityHandle {
    pub bound_addr: SocketAddr,
    pub state: Arc<ObservabilityState>,
    /// Cancels the shared token when the handle is dropped without
    /// [`Self::shutdown`]. Private, so moving `state` out stays legal.
    pub(in crate::security::observability) shutdown_on_drop: CancelOnDrop,
    // `Option` so `shutdown(self)` can TAKE the handles (a field cannot be
    // moved out of `self` directly in a method that owns it and is also
    // dropped at its end).
    pub(in crate::security::observability) listener_task: Option<JoinHandle<()>>,
    pub(in crate::security::observability) poller_task: Option<JoinHandle<()>>,
}

/// Cancels an observability server when its handle is dropped without
/// [`ObservabilityHandle::shutdown`].
///
/// A handle dropped without `shutdown()` — the ordered shutdown's future was
/// abandoned by its deadline — must still stop both tasks: they hold an
/// `Arc<ShamirDb>` and the router, and leaving them live would keep the stores
/// locked past the teardown. The awaits stay in `shutdown()`. The cancel sits
/// on this private newtype's own `Drop` rather than on `ObservabilityHandle` so
/// the handle itself stays move-partial.
pub(in crate::security::observability) struct CancelOnDrop(
    pub(in crate::security::observability) CancellationToken,
);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl ObservabilityHandle {
    /// Ask both tasks to stop WITHOUT waiting for them.
    ///
    /// The shutdown path signals this BEFORE any wait a deadline can abandon,
    /// so an expiry never depends on the handle's `Drop` having run yet.
    pub fn cancel(&self) {
        self.shutdown_on_drop.0.cancel();
    }

    /// Stop the HTTP listener and the process-metrics poller.
    ///
    /// F-68 (#895) cluster D / task #124 added timestamped `tracing` events
    /// around each of the two `JoinHandle` awaits individually, suspecting
    /// `axum::serve`'s graceful shutdown waiting on a lingering keep-alive
    /// connection (`listener_task`). Task #922 / F-68b used that
    /// instrumentation on a real ubuntu-latest CI hang
    /// (`metrics_exposes_unbounded_sentinel_when_no_byte_budget` /
    /// `metrics_exposes_finite_byte_budget_gauges`, run `30757334929`) and it
    /// disproved that hypothesis: the log showed `listener_task.await`
    /// returning in ~100µs, then hanging forever on `poller_task.await` with
    /// no further output. Root cause: `poller_task` (below, in
    /// `spawn_with_byte_budget`) used to wait on `shutdown_for_poller
    /// .notified()` inside a `tokio::select!` — `Notify::notify_waiters()`
    /// only wakes tasks that are actively polling `.notified()` AT THE
    /// MOMENT it is called; it stores no permit. Same class of race
    /// `Scheduler`'s own doc comment (`scheduler.rs`, near its
    /// `shutdown_tx` field) already names precisely: if `notify_waiters()`
    /// fires before the spawned task has been polled for the first time
    /// (not yet inside its `select!` at all) — plausible here since
    /// `tokio::spawn` only schedules the task, it doesn't run it
    /// synchronously — the notify is silently dropped and the task waits
    /// out its next `interval.tick()` (5s here) instead. The same is also
    /// true of the narrower "between loop iterations" window (each
    /// `select!` re-evaluation creates a *new* `Notified` future with its
    /// own dead zone) — either window reproduces the observed hang;
    /// `CancellationToken`'s fix below closes both by construction, so
    /// distinguishing which one fired in the specific CI repro isn't load
    /// bearing for the fix. This is the exact same lossy-`Notify` class
    /// already documented and fixed once in this crate for the root
    /// shutdown signal (see `ServerHandle::shutdown_token`'s doc comment in
    /// `server/server_handle.rs`, and `Scheduler`'s broadcast-based fix) —
    /// this call site was missed during that remediation. Fixed here by
    /// switching to `tokio_util::sync::
    /// CancellationToken`, whose `cancel()` is a persistent flag: any
    /// `.cancelled()` future (existing or newly created) resolves
    /// immediately once cancelled, closing the race by construction. The
    /// `tracing` instrumentation is kept (still useful if some other stall
    /// appears here later).
    ///
    /// # Per-side bound and result
    ///
    /// Each of the two joins is awaited under its OWN
    /// [`OBS_TASK_JOIN_TIMEOUT`], never one budget shared by both: an in-flush
    /// `/metrics` handler can keep `axum::serve` from returning while the
    /// poller has already stopped, and the old listener-then-poller sequence
    /// reported that as one "listener or poller" blur. On expiry the side is
    /// left `false` in the returned [`ObservabilityShutdownOutcome`] and the
    /// other side is STILL awaited, so the caller can name exactly which side
    /// is still draining (and may still hold the database) instead of guessing.
    pub async fn shutdown(mut self) -> ObservabilityShutdownOutcome {
        let shutdown_started = std::time::Instant::now();
        tracing::debug!("ObservabilityHandle::shutdown: enter, calling cancel()");
        self.shutdown_on_drop.0.cancel();
        tracing::debug!(
            elapsed = ?shutdown_started.elapsed(),
            "ObservabilityHandle::shutdown: cancel() returned"
        );

        let listener_wait_started = std::time::Instant::now();
        tracing::debug!("ObservabilityHandle::shutdown: awaiting listener_task");
        let listener_joined = match self.listener_task.take() {
            // A panicked task also counts as joined: it is gone and holds no
            // database reference any more.
            Some(listener_task) => {
                match tokio::time::timeout(OBS_TASK_JOIN_TIMEOUT, listener_task).await {
                    Ok(_) => true,
                    Err(_) => {
                        tracing::warn!(
                            timeout = ?OBS_TASK_JOIN_TIMEOUT,
                            "ObservabilityHandle::shutdown: metrics listener is still \
                             draining a connection; its join was abandoned (the task \
                             stops at its next cancellation point)"
                        );
                        false
                    }
                }
            }
            // Already joined, or never spawned: nothing is left to hold the db.
            None => true,
        };
        tracing::debug!(
            joined = listener_joined,
            elapsed = ?listener_wait_started.elapsed(),
            "ObservabilityHandle::shutdown: listener_task.await finished or expired"
        );

        let poller_wait_started = std::time::Instant::now();
        tracing::debug!("ObservabilityHandle::shutdown: awaiting poller_task");
        let poller_joined = match self.poller_task.take() {
            Some(poller_task) => {
                match tokio::time::timeout(OBS_TASK_JOIN_TIMEOUT, poller_task).await {
                    Ok(_) => true,
                    Err(_) => {
                        tracing::warn!(
                            timeout = ?OBS_TASK_JOIN_TIMEOUT,
                            "ObservabilityHandle::shutdown: the metrics poller is still \
                             inside a tick; its join was abandoned (the task stops at \
                             its next cancellation point)"
                        );
                        false
                    }
                }
            }
            None => true,
        };
        tracing::debug!(
            joined = poller_joined,
            elapsed = ?poller_wait_started.elapsed(),
            "ObservabilityHandle::shutdown: poller_task.await finished or expired"
        );

        tracing::debug!(
            total_elapsed = ?shutdown_started.elapsed(),
            "ObservabilityHandle::shutdown: exit"
        );
        ObservabilityShutdownOutcome {
            listener_joined,
            poller_joined,
        }
    }
}
