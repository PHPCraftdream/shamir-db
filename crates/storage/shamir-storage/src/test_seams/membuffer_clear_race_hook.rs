//! Test-only deterministic seam originally built for the `dirty_nonempty`
//! clear-race (#535 rounds 1+2). Task #539 replaced the boolean
//! `dirty_nonempty` sentinel with an inline `dirty_count: AtomicUsize`
//! cardinality mirror, which decrements at the SAME logical point an entry is
//! actually removed — there is no longer a separate "clear" step for a writer
//! to race into. This hook is kept only to preserve the existing regression
//! tests' shape: `drain_once` still calls
//! [`ClearRaceHook::at_clear_window`] right before its removal loop (a no-op
//! in production), so a test can still install a racing-writer-insert
//! callback at that point — the interleaving it now exercises no longer
//! reproduces a masked write (the counter redesign closed that structurally),
//! but the existing tests use it to prove the entry remains visible via the
//! `dirty` probe regardless of timing.
//!
//! Mirrors the `Notify`-based rendezvous style of #534's
//! `index2_backfill_hook`, but the callback shape is a plain closure because the
//! racing insert this hook drives is a synchronous `DashMap` mutation — no
//! `.await` is needed inside the window, so a `Fn` seam keeps the drain path's
//! `Send` bound clean without threading a future through it.

use std::sync::Arc;

/// A callback invoked by `drain_once` right before its removal loop (formerly
/// the `dirty_nonempty` clear-race window — see module doc for why that
/// window no longer exists after task #539's counter redesign). The
/// test-installed callback simulates a writer racing a `dirty.insert` at
/// that point.
pub(crate) type ClearRaceCallback = Arc<dyn Fn() + Send + Sync>;

/// Installable seam. Wraps an optional callback; `None` (the production /
/// default state) is a zero-overhead no-op.
#[derive(Clone, Default)]
pub(crate) struct ClearRaceHook {
    cb: Option<ClearRaceCallback>,
}

impl ClearRaceHook {
    /// Install a callback fired at the clear-race window.
    pub(crate) fn install(cb: ClearRaceCallback) -> Self {
        Self { cb: Some(cb) }
    }

    /// Fire the callback if one is installed. Called by `drain_once` right
    /// before its removal loop (see module doc — no longer a real "clear
    /// window" since task #539's counter redesign, kept for test shape).
    pub(crate) fn at_clear_window(&self) {
        if let Some(cb) = &self.cb {
            cb();
        }
    }
}

/// Parks `insert_many` between its first and second items so a test can
/// drain the first item before the second is published. The former boolean
/// sentinel was replaced by `dirty_count`; reads probe dirty unconditionally.
/// The oracle checks the count and visibility of the second item.
#[derive(Default)]
pub(crate) struct BatchInsertPauseHook {
    /// Fired once `insert_many` has parked after its first iteration.
    reached: tokio::sync::Notify,
    /// Fired by the test to let the parked `insert_many` proceed.
    resume: tokio::sync::Notify,
}

impl BatchInsertPauseHook {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Called from `insert_many` between its first and second iteration.
    /// Announces arrival, then parks until the test calls [`release`].
    pub(crate) async fn wait_after_first_item(&self) {
        self.reached.notify_one();
        self.resume.notified().await;
    }

    /// Test side: block until `insert_many` has parked.
    pub(crate) async fn wait_until_parked(&self) {
        self.reached.notified().await;
    }

    /// Test side: let the parked `insert_many` proceed to its next iteration.
    pub(crate) fn release(&self) {
        self.resume.notify_one();
    }
}
