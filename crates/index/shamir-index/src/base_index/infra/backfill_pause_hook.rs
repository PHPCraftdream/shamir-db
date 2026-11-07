//! Test-only deterministic pause hook for `IndexManager::create_index_from_records`.
//!
//! F-72 (#899, P0): mirrors `shamir-engine`'s `index2_backfill_hook`'s
//! `BackfillPauseHook` (same two-`Notify` rendezvous shape). Lets a
//! regression test park the regular-hash CREATE INDEX backfill mid-stream —
//! after the definition is registered at `state = Building` (and hence
//! invisible to every planner lookup), before it flips to `Ready` — so a
//! concurrent read issued while the create is parked can be asserted to fall
//! back to a full scan (or the prior complete state) instead of observing a
//! half-populated index.
//!
//! `shamir-engine` cannot be a dependency of `shamir-index` (the reverse
//! dependency direction), so this is a small local duplicate of the same
//! primitive rather than a shared import — kept intentionally minimal.

use tokio::sync::Notify;

/// Rendezvous between a paused backfill loop and the test driving it.
#[derive(Default)]
pub struct BackfillPauseHook {
    /// Fired by the backfill once it has reached the pause point. The test
    /// awaits this to know the window is open.
    reached: Notify,
    /// Fired by the test to let the parked backfill proceed.
    resume: Notify,
}

impl BackfillPauseHook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called from the backfill loop at the pause point. Announces arrival,
    /// then parks until the test calls [`release`](Self::release).
    pub async fn wait_at_window(&self) {
        self.reached.notify_one();
        self.resume.notified().await;
    }

    /// Test side: block until the backfill has parked in
    /// [`wait_at_window`](Self::wait_at_window).
    pub async fn wait_until_parked(&self) {
        self.reached.notified().await;
    }

    /// Test side: let the parked backfill proceed.
    pub fn release(&self) {
        self.resume.notify_one();
    }
}
