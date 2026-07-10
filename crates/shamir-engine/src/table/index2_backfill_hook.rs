//! Test-only deterministic pause hook for `create_index_v2`.
//!
//! Installed on a [`TableManager`](super::table_manager::TableManager) via the
//! `#[cfg(test)]` `create_index2_backfill_hook` field, this lets a #534
//! regression test freeze a `create_index_v2` call at the precise instant its
//! index2 backfill has finished but the new backend has NOT yet been registered
//! in the `index2_registry`. That is the exact lost-write window (finding 1):
//! a concurrent writer's row is invisible to BOTH the backfill (cursor already
//! past it) AND the live `index2_on_insert` hook (backend not yet routable).
//!
//! With the hook the test can:
//!   1. spawn `create_index_v2`, which runs the backfill, then blocks in
//!      [`wait_at_window`] (signalling `reached` first);
//!   2. drive a concurrent `insert` — which, WITH the fix, blocks on the
//!      `unique_write_lock` the paused create still holds, and WITHOUT the fix
//!      would slip through and be lost;
//!   3. release the create via [`release`], then assert the row is queryable
//!      through the new index.
//!
//! Two `Notify`s give the test full ordering control (create → "I'm parked",
//! test → "resume"), avoiding any timing-dependent `sleep` in the assertion
//! path.

use tokio::sync::Notify;

/// Rendezvous between a paused `create_index_v2` and the test driving it.
#[derive(Default)]
pub struct BackfillPauseHook {
    /// Fired by the create once it has reached the pause point (post-backfill,
    /// pre-register). The test awaits this to know the window is open.
    reached: Notify,
    /// Fired by the test to let the parked create proceed to register.
    resume: Notify,
}

impl BackfillPauseHook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called from `create_index_v2` at the post-backfill / pre-register point.
    /// Announces arrival, then parks until the test calls [`release`].
    pub async fn wait_at_window(&self) {
        self.reached.notify_one();
        self.resume.notified().await;
    }

    /// Test side: block until the create has parked in [`wait_at_window`].
    pub async fn wait_until_parked(&self) {
        self.reached.notified().await;
    }

    /// Test side: let the parked create proceed.
    pub fn release(&self) {
        self.resume.notify_one();
    }
}
