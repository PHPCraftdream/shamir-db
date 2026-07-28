//! F-48 (#859, P0) — reusable writer-drain barrier.
//!
//! Closes the check-then-act race that `unique_write_lock` + an intent flag
//! alone cannot close. A writer that reads `needs_write_barrier() == false`
//! proceeds lock-free through its ENTIRE validate→write→index sequence with
//! no further check; the DDL/index-create side raises the flag and takes the
//! SAME lock, but never waits for (drains) a writer that already read `false`
//! a moment earlier. This module provides a small, reusable primitive that
//! genuinely drains those in-flight fast-path writers before the drainer
//! proceeds past its snapshot/proof point.
//!
//! See `table_manager_index_mgmt.rs::backfill_index2_backend`'s doc comment
//! ("Check-then-act, not a drain") for the older, candidly-documented
//! instance of the SAME race class — F-50 will wire this SAME primitive into
//! `create_index_v2`'s residual with no new design work.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Reusable drain barrier for a table's non-tx write path.
///
/// One instance lives on each
/// [`TableManager`](super::table_manager::TableManager) (shared across clones
/// via `Arc`, like the sibling barrier flags). Fast-path writers — those that
/// read `needs_write_barrier() == false` — bracket their entire
/// validate→write→index sequence with [`enter_writer`](Self::enter_writer)
/// (RAII bump/decrement of an `active` counter). A drainer — after raising its
/// intent flag AND acquiring `unique_write_lock` — calls
/// [`drain`](Self::drain) to wait for every in-flight fast-path writer to
/// exit before proceeding.
///
/// # Why bump BEFORE the flag check
///
/// The counter increment MUST be sequenced before the
/// `needs_write_barrier()` flag read in the writer. This lets the flag's
/// coherence ordering carry the happens-before: if a writer reads `false`
/// (flag not yet up) and proceeds, the drainer's drain — called after the
/// flag's `Release` store — observes the writer's increment via the coherence
/// chain
/// (`writer.fetch_add` sb→ `flag.load == false` coherence-ordered-before→
/// `flag.store == true` sb→ `drain.load`). Reversing the order (flag check
/// then bump) reopens the race: the drain could load `0` in the gap between
/// the flag read and the bump, then proceed while the writer is still
/// in flight.
///
/// # Slow-path writers do NOT stay in the drain set
///
/// A writer that reads `needs_write_barrier() == true` takes the slow path
/// (`unique_write_lock`). It MUST exit the drain set (drop the guard) BEFORE
/// blocking on the lock — otherwise the drainer (which holds the lock) would
/// wait forever for a writer that cannot make progress. Slow-path writers are
/// serialized by the lock itself and cannot be in flight when
/// [`drain`](Self::drain) returns.
///
/// # Cost
///
/// `enter_writer`: one `Relaxed fetch_add` (uncontended cache-line RMW,
/// ~ns). `WriterDrainGuard::drop`: one `Release fetch_sub`. `drain`: one
/// `Acquire` load per iteration, `yield_now` between iterations (rare DDL
/// path). When no drain is in progress the drainer never touches this
/// primitive — zero contention.
///
/// # Memory model
///
/// `enter_writer`'s `Relaxed fetch_add` is sufficient because the flag's
/// coherence chain (above) carries the happens-before edge to the drain load
/// — no additional ordering on the increment itself is needed.
/// `WriterDrainGuard::drop`'s `Release fetch_sub` pairs with the drainer's
/// `Acquire` load so the writer's pre-decrement work (data-store write,
/// record-counter bump, index updates) happens-before the drainer's
/// post-drain work (the `keyset_safe` count-proof / index backfill
/// snapshot). The data store's own internal synchronization provides
/// cross-thread write visibility; this counter provides temporal drain — no
/// fast-path writer is still in flight when `drain()` returns — mirroring the
/// role `unique_write_lock` plays on the slow path.
///
/// # Reusability (F-50)
///
/// This primitive is barrier-agnostic: [`drain`](Self::drain) waits for ALL
/// in-flight fast-path writers, regardless of which intent flag the drainer
/// raised. F-50 will call `TableManager::drain_writers()` (which delegates
/// here) from `create_index_v2` — after raising `index2_create_barrier`,
/// before the backfill snapshot — with no new design work, exactly the way
/// F-48 calls it from `SchemaActivationBarrierGuard::raise` for the
/// schema-activation DDL.
#[derive(Debug)]
pub struct WriterDrainBarrier {
    active: Arc<AtomicUsize>,
}

impl WriterDrainBarrier {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Enter the drain set: bump the active-writer counter, return an RAII
    /// guard that decrements on drop.
    ///
    /// Call this on the fast path BEFORE reading `needs_write_barrier()`,
    /// bracketing the entire validate→write→index sequence. If the flag check
    /// then returns `true` (slow path), drop the returned guard BEFORE taking
    /// `unique_write_lock` — the lock serializes the slow path, the counter
    /// must not.
    #[must_use]
    pub fn enter_writer(&self) -> WriterDrainGuard {
        self.active.fetch_add(1, Ordering::Relaxed);
        WriterDrainGuard {
            active: Arc::clone(&self.active),
        }
    }

    /// Drain: wait until every in-flight fast-path writer has exited the
    /// drain set.
    ///
    /// The caller MUST have already (1) raised its intent flag (`Release`)
    /// so NEW writers take the slow (locked) path, and (2) hold
    /// `unique_write_lock` so slow-path writers are blocked. Then this
    /// catches any writer that read `false` before the flag went up.
    ///
    /// When no writers are active, returns after a single `Acquire` load.
    pub async fn drain(&self) {
        while self.active.load(Ordering::Acquire) != 0 {
            tokio::task::yield_now().await;
        }
    }

    /// Test-only: current active-writer count (for assertions).
    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

impl Clone for WriterDrainBarrier {
    fn clone(&self) -> Self {
        Self {
            active: Arc::clone(&self.active),
        }
    }
}

impl Default for WriterDrainBarrier {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard returned by [`WriterDrainBarrier::enter_writer`]. Decrements
/// the counter on drop (Release), releasing drain-set membership.
#[derive(Debug)]
pub struct WriterDrainGuard {
    active: Arc<AtomicUsize>,
}

impl Drop for WriterDrainGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_returns_immediately_when_no_writers_active() {
        let b = WriterDrainBarrier::new();
        assert_eq!(b.active_count(), 0);
        // Must not hang — single Acquire load, immediate return.
        b.drain().await;
    }

    #[tokio::test]
    async fn drain_waits_until_all_writers_exit() {
        let b = WriterDrainBarrier::new();
        let g1 = b.enter_writer();
        let g2 = b.enter_writer();
        assert_eq!(b.active_count(), 2);

        // drain must block while writers are active. Spawn it and confirm it
        // does not finish.
        let b2 = WriterDrainBarrier::clone(&b);
        let drain = tokio::spawn(async move { b2.drain().await });
        tokio::task::yield_now().await;
        assert!(!drain.is_finished(), "drain must wait for active writers");

        drop(g1);
        tokio::task::yield_now().await;
        assert!(
            !drain.is_finished(),
            "drain must wait until ALL writers exit"
        );

        drop(g2);
        drain.await.expect("drain completes once counter hits 0");
    }

    #[tokio::test]
    async fn guard_decrement_is_release_paired_with_drain_acquire() {
        // Structural: enter + drop returns the counter to 0 so a subsequent
        // drain observes it (Acquire load reads the Release-stored 0).
        let b = WriterDrainBarrier::new();
        {
            let _g = b.enter_writer();
            assert_eq!(b.active_count(), 1);
        }
        assert_eq!(b.active_count(), 0);
        b.drain().await;
    }

    #[tokio::test]
    async fn clone_shares_the_same_counter() {
        // Clones must observe the same counter (mirrors TableManager's
        // Arc-shared barrier flags) — a writer on one clone must be visible
        // to a drain on another.
        let a = WriterDrainBarrier::new();
        let b = a.clone();
        let _g = a.enter_writer();
        assert_eq!(
            b.active_count(),
            1,
            "clone must share the same Arc<AtomicUsize>"
        );
    }
}
