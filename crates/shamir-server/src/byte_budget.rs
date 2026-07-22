//! Global byte-budget primitive — RI-15.
//!
//! `tokio::sync::Semaphore` counts *permits*, not bytes, and its permit
//! count is a `u32` — capping any byte-granular reuse of it at `u32::MAX`
//! and coupling byte accounting to permit accounting. [`ByteBudget`] is a
//! purpose-built async byte accountant: an `AtomicUsize` CAS-loop fast path
//! for acquire/release plus a [`tokio::sync::Notify`] to wake waiters when
//! bytes free up.
//!
//! # Why this exists
//!
//! The server already clamps the size of any ONE batch response
//! (`security.query_limits.max_result_size_bytes`,
//! `crates/shamir-server/src/db_handler/handler.rs`), but nothing bounded
//! the SUM of in-flight response bytes across every concurrently-executing
//! batch/connection. At `max_active_connections = 1000` × a 64 MiB
//! per-batch cap, worst case is ~64 GiB of simultaneously buffered response
//! memory — unbounded relative to a typical 4–8 GiB container. `ByteBudget`
//! closes that gap: a single server-wide cap on in-flight response bytes.
//!
//! # Usage
//!
//! `ShamirDbHandler::execute` acquires a [`ByteBudgetGuard`] for the actual
//! serialized size of the response it is about to return (constraint: the
//! gate reserves the REAL size, not the `max_result_size_bytes` upper
//! bound — reserving the cap upfront would under-utilize the budget by the
//! cap-to-actual ratio). The guard is threaded through
//! `connection::request_loop::WriterMsg::{Reply,ReplyAndClose}` and dropped
//! only after the writer task finishes the socket write (success or
//! error) — so the accounted bytes stay "reserved" for exactly as long as
//! they occupy memory on the write path, and the release always happens on
//! the writer task, never the dispatch task.
//!
//! # Fairness
//!
//! Waiters are NOT served strict FIFO. Every waiter wakes on every
//! `notify_waiters()` call (broadcast) and independently retries the CAS;
//! whichever wins the race proceeds, the rest re-park. This gives
//! **at-least-one-progress** per release (a release that frees enough
//! bytes for the head-of-line waiter is guaranteed to unblock SOMEONE, but
//! not necessarily in arrival order). Acceptable here: the budget is a
//! coarse OOM shield, not a scheduler — starvation would require a
//! continuous stream of large releases each claimed by a newer waiter,
//! which does not happen under the realistic workload (a fixed pool of
//! `max_active_connections` each issuing one batch at a time).

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// Shared state behind [`ByteBudget`]'s `Arc`. Split out so [`ByteBudget`]
/// itself can be a cheap `Clone` (just bumps the `Arc` refcount).
struct Inner {
    /// Bytes currently reserved by outstanding [`ByteBudgetGuard`]s.
    used: AtomicUsize,
    /// Hard cap in bytes. `None` = unbounded (preserves pre-RI-15
    /// behavior: `acquire` always succeeds immediately).
    cap: Option<usize>,
    /// Wakes every waiter parked in `acquire` when bytes are released.
    notify: Notify,
}

/// Server-wide async byte accountant. Cheaply `Clone` (wraps an `Arc`).
///
/// Construct once at boot (`server_launcher.rs`, next to
/// `QueryLimitsCap`) from `security.query_limits.max_inflight_response_bytes`
/// and share the single instance across every connection/handler clone.
#[derive(Clone)]
pub struct ByteBudget {
    inner: Arc<Inner>,
}

impl ByteBudget {
    /// New budget with the given cap. `None` means unbounded: every
    /// `acquire` returns immediately regardless of `bytes`, matching the
    /// pre-RI-15 behavior (no global gate).
    pub fn new(cap: Option<usize>) -> Self {
        Self {
            inner: Arc::new(Inner {
                used: AtomicUsize::new(0),
                cap,
                notify: Notify::new(),
            }),
        }
    }

    /// Unbounded budget — every `acquire` succeeds immediately. Used as the
    /// default for handlers/tests that don't care about the global cap.
    pub fn unbounded() -> Self {
        Self::new(None)
    }

    /// Reserve `bytes` from the budget, waiting if the cap is currently
    /// exhausted. Returns a [`ByteBudgetGuard`] that releases the
    /// reservation on `Drop` (covers every early-return / error / panic
    /// unwind path on the caller's side).
    ///
    /// `bytes` larger than the configured cap is still admitted (once the
    /// budget is fully drained) rather than deadlocking forever — a single
    /// oversized-but-otherwise-valid response should not be able to wedge
    /// the whole gate. Validation at config-load time
    /// (`max_inflight_response_bytes >= max_result_size_bytes`) is what
    /// actually keeps this case from happening in practice.
    pub async fn acquire(&self, bytes: usize) -> ByteBudgetGuard {
        let Some(cap) = self.inner.cap else {
            // Unbounded — no accounting needed, no waiting.
            return ByteBudgetGuard {
                inner: None,
                bytes: 0,
            };
        };

        loop {
            // Register interest in the next notification BEFORE re-checking
            // the CAS. This is `tokio::sync::Notify`'s documented race-free
            // pattern: `notified()` returns a future that, once polled,
            // stores itself as a waiter — so a `notify_waiters()` call that
            // happens after this line (even before we `.await` it below) is
            // not lost. Without this ordering there is a lost-wakeup window
            // between "CAS failed" and "start waiting" where a concurrent
            // release could notify into the void.
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            // Polling once here (via `enable()`) commits this waiter into
            // Notify's internal list even before the first `.await` below —
            // required so a release racing right after this line still
            // wakes us instead of being missed.
            notified.as_mut().enable();

            // Fast path: CAS loop, no lock, no waiter registration.
            let mut current = self.inner.used.load(Ordering::Acquire);
            loop {
                let after = current.saturating_add(bytes);
                // Admit if there's room, OR if the budget is currently
                // empty (current == 0) — guarantees an oversized request
                // eventually gets a turn instead of deadlocking forever
                // when `bytes > cap`.
                if after <= cap || current == 0 {
                    match self.inner.used.compare_exchange_weak(
                        current,
                        after,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            return ByteBudgetGuard {
                                inner: Some(self.inner.clone()),
                                bytes,
                            };
                        }
                        Err(actual) => {
                            current = actual;
                            continue;
                        }
                    }
                }
                break;
            }

            // Not enough room right now — park until a release notifies us
            // (the `enable()` above guarantees we don't miss a release that
            // happened between the CAS failure and this await), then retry
            // the whole loop: state may have changed again by the time we
            // wake, so we re-check rather than assume we now have room.
            notified.await;
        }
    }

    /// Current reserved-byte count (0 if unbounded/no outstanding guards).
    pub fn used(&self) -> usize {
        self.inner.used.load(Ordering::Acquire)
    }

    /// Configured cap, or `None` if unbounded.
    pub fn cap(&self) -> Option<usize> {
        self.inner.cap
    }
}

/// RAII reservation. Releases its `bytes` back to the [`ByteBudget`] on
/// `Drop` and wakes any parked waiters — fires on every path (normal
/// completion, write error, task abort/panic unwind).
///
/// Deliberately holds `Arc<Inner>` directly (not `ByteBudget`) so it has no
/// dependency on the public wrapper type — keeps the guard `Send + 'static`
/// with the smallest possible footprint, since it rides inside
/// `connection::request_loop::WriterMsg` across an `mpsc` channel.
pub struct ByteBudgetGuard {
    inner: Option<Arc<Inner>>,
    bytes: usize,
}

impl Drop for ByteBudgetGuard {
    fn drop(&mut self) {
        if let Some(inner) = &self.inner {
            if self.bytes > 0 {
                inner.used.fetch_sub(self.bytes, Ordering::AcqRel);
            }
            // Broadcast: every currently-parked `acquire` re-checks the CAS.
            // Cheap even with zero waiters (Notify short-circuits).
            inner.notify.notify_waiters();
        }
    }
}

tokio::task_local! {
    /// Side-channel that carries a just-acquired [`ByteBudgetGuard`] out of
    /// `db_handler::handler::ShamirDbHandler::execute` to the dispatch task
    /// in `connection::request_loop`, without changing
    /// `shamir_connect::server::dispatch::RequestHandler::handle`'s
    /// `Result<Vec<u8>, String>` signature (that trait lives in a different
    /// crate and carries no per-request resource context — see the RI-15
    /// brief).
    ///
    /// Both the writer (`execute`, via [`stash_guard`]) and the reader
    /// (`connection::request_loop`, via [`take_stashed_guard`]) run inside
    /// the SAME spawned per-request dispatch task
    /// (`request_loop.rs::join_set.spawn(async move { ... })`), so this
    /// task-local is never shared across concurrent requests — each
    /// request's task gets its own independent storage slot the moment it
    /// is `.scope()`d in `run_with_guard_slot`.
    pub(crate) static PENDING_RESPONSE_BUDGET_GUARD: RefCell<Option<ByteBudgetGuard>>;
}

/// Run `fut` with a fresh, empty [`PENDING_RESPONSE_BUDGET_GUARD`] slot
/// scoped to it. Call this once per dispatched request, wrapping the same
/// future that (transitively) calls `ShamirDbHandler::execute`.
pub async fn run_with_guard_slot<F: std::future::Future>(fut: F) -> F::Output {
    PENDING_RESPONSE_BUDGET_GUARD
        .scope(RefCell::new(None), fut)
        .await
}

/// Called from inside `ShamirDbHandler::execute` right after acquiring the
/// budget for the response about to be returned. Overwrites any previously
/// stashed guard for this request (there is at most one response per
/// dispatched request, so this is always a fresh slot).
///
/// Outside a [`run_with_guard_slot`] scope (e.g. a unit test that calls
/// `ShamirDbHandler::execute` directly without going through
/// `request_loop`) this is a silent no-op: the guard is simply dropped
/// immediately, releasing the budget right away instead of holding it
/// through a write that will never happen. That is a safe fallback — it
/// only under-holds the budget in a context that was never going to route
/// the response through the real writer anyway.
pub(crate) fn stash_guard(guard: ByteBudgetGuard) {
    let _ = PENDING_RESPONSE_BUDGET_GUARD.try_with(|cell| {
        *cell.borrow_mut() = Some(guard);
    });
}

/// Called from `connection::request_loop` immediately after the dispatch
/// future resolves, to retrieve (and take ownership of) the guard
/// `execute` stashed for this response — or `None` if the budget is
/// unbounded, the request never reached `execute` (e.g. `Ping`,
/// `CreateScramUser`), or `execute` short-circuited before running the
/// batch (version/permission/read-only gates).
pub(crate) fn take_stashed_guard() -> Option<ByteBudgetGuard> {
    PENDING_RESPONSE_BUDGET_GUARD
        .try_with(|cell| cell.borrow_mut().take())
        .ok()
        .flatten()
}
