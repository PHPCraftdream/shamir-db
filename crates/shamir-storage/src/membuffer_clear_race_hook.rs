//! Test-only deterministic seam for the `dirty_nonempty` clear-race (#535).
//!
//! `MemBufferStore::drain_once` clears the `dirty_nonempty` fast-path sentinel
//! at the very end, once the dirty buffer is observed empty. The clear is a
//! two-step, non-atomic sequence: `dirty.is_empty()` (observed `true`) THEN
//! `dirty_nonempty.store(false, ...)`. A writer's `dirty.insert` (with its own
//! preceding `dirty_nonempty.store(true)`) can land in the gap between those two
//! steps — leaving a real, ACKed entry in `dirty` while the sentinel reads
//! `false`, masking the write from every subsequent `get()` /
//! `snapshot_overlay_sorted()` fast-path check.
//!
//! Reproducing that race deterministically (no `sleep`-based timing luck)
//! requires a seam at the exact instant the clear is about to happen. This hook
//! is that seam: `drain_once` calls [`ClearRaceHook::at_clear_window`] AFTER it
//! has observed `dirty.is_empty()` but BEFORE the `store(false)` that follows —
//! i.e. exactly in the gap the bug depends on. A test installs a hook whose
//! callback performs the racing writer insert at that precise point, forcing
//! the exact interleaving the bug depends on.
//!
//! Mirrors the `Notify`-based rendezvous style of #534's
//! `index2_backfill_hook`, but the callback shape is a plain closure because the
//! racing insert this hook drives is a synchronous `DashMap` mutation — no
//! `.await` is needed inside the window, so a `Fn` seam keeps the drain path's
//! `Send` bound clean without threading a future through it.

use std::sync::Arc;

/// A callback invoked by `drain_once` after it has observed `dirty.is_empty()`
/// but BEFORE it stores `false` into `dirty_nonempty` (the clear-race window).
/// The test-installed callback simulates a writer whose `dirty.insert` raced
/// into the gap.
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

    /// Fire the callback if one is installed. Called by `drain_once` at the
    /// clear-race window (post-`is_empty()` observation, pre-`store(false)`).
    pub(crate) fn at_clear_window(&self) {
        if let Some(cb) = &self.cb {
            cb();
        }
    }
}

/// Task #535 round 2: deterministic test seam for the NARROWER gap an `@fl`
/// adversarial pass found in round 1's fix — a writer that publishes
/// `dirty_nonempty.store(true)` then STALLS across an `.await` before its own
/// `dirty.insert()` completes (e.g. mid-loop in `insert_many`/`set_many`/
/// `remove_many`, which yield at `cache.insert(...).await` every iteration).
/// If `drain_once`'s clear-and-verify sequence runs entirely inside that
/// stall (both `is_empty()` checks see the map without this writer's
/// not-yet-landed entry), round 1's verify-after-clear has nothing to
/// observe and restore — the writer's LATER insert then lands with the
/// sentinel stuck `false`. Closed on the writer side by republishing
/// `store(true)` immediately after each `dirty.insert()`, not just before.
///
/// This hook parks `insert_many` between its first and second loop
/// iteration (after item 0's `dirty.insert` + round-2 republish + cache
/// write, before item 1's `dirty.insert`) so a test can drive a real
/// `drain_once` into that exact window, then release the writer and prove
/// item 1 (inserted AFTER the drain already cleared the sentinel) is still
/// visible thanks to its own post-insert republish.
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
