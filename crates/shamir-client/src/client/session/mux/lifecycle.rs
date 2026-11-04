//! Lifecycle state for the rid-demux mux: pending-request and close state,
//! plus the reader task's shutdown guard.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::{oneshot, watch};

use shamir_collections::THasher;

use crate::client::subscription::{SubsMap, SubscriptionRegistry};
use crate::error::{ClientError, CloseCause};

#[cfg(test)]
use super::hooks::HookPoint;

/// Per-pending-request slot: either resolved with response bytes or with a
/// transport-level error.
pub(crate) type PendingSender = oneshot::Sender<Result<Vec<u8>, ClientError>>;

/// Monotonic close state (RV-CLIENT-B80C0D09): the cause is stored before
/// the flag is raised, so readers observing the flag see the cause. The flag
/// store and load are `SeqCst`: the pending and subscription registries both
/// publish-then-drain on shutdown and insert-then-recheck on registration, and
/// only a single total order over the flag rules out the store-buffering
/// outcome in which the drain misses an entry AND the registrant misses the
/// flag.
#[derive(Default)]
pub(crate) struct ClosedState {
    flag: AtomicBool,
    cause: OnceLock<CloseCause>,
}

impl ClosedState {
    /// Publish the terminal cause. Returns `true` only for the FIRST caller
    /// (all later causes are dropped — first cause wins), making exactly one
    /// [`Lifecycle::shutdown`] invocation the single drainer of both
    /// registries.
    pub(crate) fn publish(&self, cause: CloseCause) -> bool {
        if self.cause.set(cause).is_err() {
            return false;
        }
        self.flag.store(true, Ordering::SeqCst);
        true
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// The stored cause; `Some` iff closed. The `SeqCst` flag load observes
    /// the store that follows `OnceLock::set`, so the `get` below always finds
    /// the cause once `is_closed()` held.
    pub(crate) fn cause(&self) -> Option<CloseCause> {
        if !self.is_closed() {
            return None;
        }
        Some(
            self.cause
                .get()
                .cloned()
                .unwrap_or(CloseCause::ReaderCancelled),
        )
    }
}

/// Pending-request registry: request id → oneshot sender. `scc::HashMap` uses
/// internal bucket synchronization. Close-vs-register protocol: the cause is
/// published BEFORE the drain, so a registrant re-reads
/// `closed.cause()` AFTER a successful insert — an insert that landed before
/// the drain is resolved by it; one that lands after removes its own entry
/// and fails with the published cause.
pub(crate) struct PendingRegistry {
    pub(crate) closed: Arc<ClosedState>,
    pub(crate) map: scc::HashMap<u32, PendingSender, THasher>,
}

/// Pending-request registry handle.
pub(crate) type PendingMap = Arc<PendingRegistry>;

/// The ONE closeable lifecycle state shared by request registration,
/// subscription registration and teardown (P0 C1). `closed_hint` is a FAST
/// PATH ONLY hint (not authoritative); the authoritative lifecycle cause is
/// the `ClosedState` shared by the pending and subscription registries.
#[derive(Clone)]
pub(crate) struct Lifecycle {
    pub(crate) pending: PendingMap,
    pub(crate) subs: SubsMap,
    /// FAST PATH ONLY hint raised by [`Lifecycle::shutdown`]. The
    /// authoritative state is the shared `ClosedState` plus the registry
    /// entries.
    pub(crate) closed_hint: Arc<AtomicBool>,
    /// Closure broadcast (P0 write-phase race): `Some(cause)` once shutdown
    /// ran. Lets waiters blocked OUTSIDE the registry locks (writer
    /// acquisition, in-flight `write_frame`) unblock on the terminal
    /// transition.
    pub(crate) closed_tx: Arc<watch::Sender<Option<CloseCause>>>,
}

impl Lifecycle {
    pub(crate) fn new() -> Self {
        let closed = Arc::new(ClosedState::default());
        Self {
            pending: Arc::new(PendingRegistry {
                closed: Arc::clone(&closed),
                map: scc::HashMap::with_hasher(THasher::default()),
            }),
            subs: Arc::new(SubscriptionRegistry::new(Arc::clone(&closed))),
            closed_hint: Arc::new(AtomicBool::new(false)),
            closed_tx: Arc::new(watch::Sender::new(None)),
        }
    }

    /// The stored close cause, if the lifecycle is closed.
    pub(crate) fn cause(&self) -> Option<CloseCause> {
        self.pending.closed.cause()
    }

    /// Resolves with the close cause once [`Lifecycle::shutdown`] has run.
    /// Used to race blocking phases of `roundtrip` (writer acquisition,
    /// `write_frame`) against closure.
    pub(crate) async fn closed_wait(&self) -> CloseCause {
        let mut rx = self.closed_tx.subscribe();
        // `shutdown` publishes via `send_replace` (stored unconditionally), so
        // the cause may ALREADY be here if shutdown ran before we subscribed —
        // check synchronously first to avoid a lost wakeup.
        if let Some(cause) = rx.borrow_and_update().clone() {
            return cause;
        }
        let cause = match rx.wait_for(|c| c.is_some()).await {
            Ok(cause) => cause.clone().unwrap_or(CloseCause::ReaderCancelled),
            // Sender gone: every Lifecycle clone dropped — nothing can close.
            Err(_) => CloseCause::ReaderCancelled,
        };
        cause
    }

    /// Idempotent terminal transition; the FIRST cause wins. Subscription
    /// shutdown sends each terminal cause, then drains route/buffer state;
    /// the cause is published before the drain, so a registration or push
    /// that lands after the drain visited its key removes its own residue.
    pub(crate) fn shutdown(&self, cause: CloseCause) {
        if !self.pending.closed.publish(cause.clone()) {
            // Already closed — first cause wins, nothing to do.
            return;
        }
        self.subs.shutdown_drain(&cause);
        self.closed_tx.send_replace(Some(cause.clone()));
        self.closed_hint.store(true, Ordering::Release);
        let mut waiters: Vec<PendingSender> = Vec::new();
        self.pending.map.retain_sync(|_, tx| {
            // Move the sender out (oneshot::Sender is not Clone): leave a
            // fresh throwaway channel behind — the entry is removed anyway.
            let (dummy, _) = oneshot::channel();
            waiters.push(std::mem::replace(tx, dummy));
            false
        });
        for tx in waiters {
            let _ = tx.send(Err(ClientError::ConnectionClosed(cause.clone())));
        }
    }
}

/// Drop guard installed by [`super::reader::reader_task`] BEFORE its read loop. On any exit
/// path that did not set a cause explicitly (EOF / I/O error do) — abort,
/// cancel, panic — it shuts the lifecycle down with
/// [`CloseCause::ReaderCancelled`].
pub(crate) struct ShutdownGuard {
    state: Lifecycle,
    cause: Cell<Option<CloseCause>>,
}

impl ShutdownGuard {
    pub(crate) fn new(state: Lifecycle) -> Self {
        Self {
            state,
            cause: Cell::new(Some(CloseCause::ReaderCancelled)),
        }
    }

    pub(crate) fn set(&self, cause: CloseCause) {
        self.cause.set(Some(cause));
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        let cause = self.cause.take().unwrap_or(CloseCause::ReaderCancelled);
        self.state.shutdown(cause);
    }
}

/// Test-only lifecycle seam (P0 C1): pause points fired BY the production
/// code paths. The TYPE exists in both builds (production call sites pass
/// `LifecycleHooks::default()`), but every FIELD — and [`HookPoint`] — is
/// `#[cfg(test)]`, so nothing hook-related compiles into production builds.
#[derive(Clone, Default)]
pub(crate) struct LifecycleHooks {
    /// Reader loop ended, before the terminal shutdown transition.
    #[cfg(test)]
    pub(crate) reader_before_shutdown: Option<HookPoint>,
    /// `roundtrip` passed the fast-path closed check.
    #[cfg(test)]
    pub(crate) after_fast_check: Option<HookPoint>,
    /// `roundtrip` registered its pending sender under the registry lock.
    #[cfg(test)]
    pub(crate) after_register: Option<HookPoint>,
    /// `roundtrip` acquired the writer and passed the authoritative recheck.
    #[cfg(test)]
    pub(crate) after_writer_acquired: Option<HookPoint>,
}
