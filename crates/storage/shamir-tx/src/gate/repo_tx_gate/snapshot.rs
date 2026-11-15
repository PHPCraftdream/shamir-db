//! Snapshot registration: the RAII guard, the A10 opening barrier, and the GC floor.
use super::RepoTxGate;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    static MIN_ALIVE_PROBE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn min_alive_probe() -> u64 {
    MIN_ALIVE_PROBE.with(std::cell::Cell::get)
}

/// If the snapshot was opened for a Serializable transaction, also
/// decrements `active_serializable_count` so non-tx writes stop paying
/// the SSI-footprint overhead once no Serializable tx is watching.
pub struct SnapshotGuard {
    /// The versions this guard has registered in `active_snapshots`,
    /// paired with whether the registration used a refcount bump
    /// (always true under the refcount scheme). On drop, each entry's
    /// refcount is decremented, and the key is removed when it hits
    /// zero. A guard may carry MORE than one version when the floor
    /// moved during `open_snapshot` — the stale registration is cleaned
    /// up here rather than mid-open so there is never a zero-registration
    /// window for a concurrently-racing vacuum.
    registered: Vec<u64>,
    snapshots: Arc<scc::HashMap<u64, u64, THasher>>,
    /// Mirror of `snapshots` keys, maintained for `min_alive`'s bounded
    /// minimum lookup; entries removed exactly when the refcount hits zero.
    ///
    /// LOCK ORDER (map+tree atomicity): the hash-map bucket entry lock is
    /// held across the tree-index update for the same version; the tree is
    /// NEVER touched for a version whose map entry is not currently locked —
    /// this makes map+tree mutations atomic per version and closes the
    /// drop/open handoff race.
    versions: Arc<scc::TreeIndex<u64, ()>>,
    #[cfg(test)]
    drop_hook: Option<Box<dyn FnOnce(bool) + Send>>,
    /// Non-None iff this guard was opened for a Serializable tx.
    /// Holds the shared counter so Drop can decrement it without
    /// holding a reference back to the `RepoTxGate`.
    serializable_count: Option<Arc<AtomicU64>>,
}

impl SnapshotGuard {
    /// The snapshot version this guard protects — the FINAL (highest)
    /// registered version, which is the version the reader reads at.
    pub fn version(&self) -> u64 {
        *self.registered.last().unwrap_or(&0)
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        // Decrement the refcount for every registered version; remove
        // the entry when the count reaches zero. Each remove runs under
        // the per-entry exclusive lock (scc entry API), so the read-
        // modify-write is race-free.
        for &v in &self.registered {
            // Use the sync `entry` API — Drop is not async. `entry` takes
            // the per-entry exclusive lock, so we can read-and-decrement
            // atomically.
            match self.snapshots.entry_sync(v) {
                scc::hash_map::Entry::Occupied(mut e) => {
                    let count = e.get_mut();
                    if *count <= 1 {
                        // LOCK ORDER: remove from the tree FIRST, while the
                        // OccupiedEntry `e` still holds the hash-map bucket
                        // entry lock; only then consume the map entry. The
                        // bucket lock is held across the tree-index update
                        // for the same version — the tree is NEVER touched
                        // for a version whose map entry is not currently
                        // locked — making map+tree mutations atomic per
                        // version and closing the drop/open handoff race
                        // (a new opener of the same version can never find
                        // the tree key deleted out from under its live pin).
                        let _ = self.versions.remove_sync(&v);
                        #[cfg(test)]
                        if let Some(hook) = self.drop_hook.take() {
                            hook(self.versions.contains(&v));
                        }
                        let _ = e.remove_entry();
                    } else {
                        *count -= 1;
                    }
                }
                scc::hash_map::Entry::Vacant(_) => {
                    // Already removed by a prior drop or never inserted —
                    // safe to ignore (idempotent).
                }
            }
        }
        if let Some(cnt) = &self.serializable_count {
            cnt.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// A10 in-flight barrier RAII guard. Increments the counter on creation,
/// decrements on drop (including cancellation). Ensures vacuum's fast path
/// never deletes a version while a reader is between its floor-read and
/// registration completion.
pub(crate) struct OpeningBarrier<'a> {
    counter: &'a AtomicU64,
}

impl<'a> OpeningBarrier<'a> {
    fn new(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl<'a> Drop for OpeningBarrier<'a> {
    fn drop(&mut self) {
        // AcqRel: the decrement must be visible to vacuum's Acquire load
        // ONLY AFTER the registration that preceded this drop is complete.
        // fetch_sub returns the previous value; we only assert it was > 0
        // in debug builds to catch double-decrement bugs.
        let prev = self.counter.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "OpeningBarrier double-decrement");
    }
}

/// Result of waiting for the contiguous visibility prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleOutcome {
    Visible,
    ReopenRequired,
}

impl RepoTxGate {
    /// cancel-safe: yes — the `entry_sync` calls in `bump_refcount`
    /// (formerly `entry_async`; switched to the synchronous accessor to
    /// close the same-class whole-runtime deadlock hazard fixed for the
    /// `cells` map in #589 / commit `7a4abf62`) are CAS-based and either
    /// complete or leave the map unchanged on cancellation. The function
    /// no longer suspends. If the future is dropped after a registration
    /// but before returning the guard, the snapshot entry leaks (no Drop
    /// runs); since callers always await this to completion or never call
    /// it, in practice this is cancel-safe.
    ///
    /// Register Snapshot/Serializable reads at the contiguous completion watermark W.
    /// Eager publication can skip unsettled versions, which would later appear
    /// inside a snapshot. Latest-state reads still use `last_committed`.
    /// Registration is protected by the A10 opening barrier and rechecked
    /// before returning the RAII guard.
    ///
    /// Use [`open_snapshot_serializable`](Self::open_snapshot_serializable)
    /// when the transaction's isolation level is `Serializable`.
    pub async fn open_snapshot(&self) -> SnapshotGuard {
        let registered = self.register_snapshot().await;
        // The snapshot version the reader reads at is the FINAL (highest)
        // registered version — the floor can only move forward.
        SnapshotGuard {
            registered,
            snapshots: Arc::clone(&self.active_snapshots),
            versions: Arc::clone(&self.snapshot_versions),
            #[cfg(test)]
            drop_hook: None,
            serializable_count: None,
        }
    }

    /// Register exactly `pin`, without reconciling it with the published
    /// marker or the contiguous completion watermark W.
    ///
    /// Intended for DDL callers that establish a per-table cut under a
    /// writer-drain barrier. The caller must ensure the requested history
    /// has not already been reclaimed before registration begins.
    ///
    /// Uses the A10 opening barrier and the same refcount/Drop ownership as
    /// `open_snapshot`. A pin above W must not become the GC minimum by
    /// itself: GC must also include W in its floor (the separate GC-floor
    /// remediation). This method does not settle versions or advance W.
    ///
    /// cancel-safe: yes — `bump_refcount` has no suspension points, so the
    /// opening barrier, registration, and guard construction finish in one
    /// poll. Dropping an unpolled future registers nothing.
    pub async fn open_snapshot_at(&self, pin: u64) -> SnapshotGuard {
        let _barrier = OpeningBarrier::new(&self.active_snapshots_opening);
        // Allocate before registration so the guard owns every bumped refcount.
        let registered = vec![pin];
        self.bump_refcount(pin).await;
        SnapshotGuard {
            registered,
            snapshots: Arc::clone(&self.active_snapshots),
            versions: Arc::clone(&self.snapshot_versions),
            #[cfg(test)]
            drop_hook: None,
            serializable_count: None,
        }
    }

    /// Like [`open_snapshot`](Self::open_snapshot) but also increments
    /// `active_serializable_count` for the lifetime of the guard.
    ///
    /// Must be called instead of `open_snapshot` when opening a snapshot
    /// for a `Serializable` transaction so that non-tx writes know at
    /// least one Serializable observer is alive and must record their
    /// SSI footprint.
    ///
    /// cancel-safe: same guarantee as `open_snapshot`.
    pub async fn open_snapshot_serializable(&self) -> SnapshotGuard {
        let registered = self.register_snapshot().await;
        // Increment AFTER the snapshot version is registered so that
        // any concurrent non-tx write that races with this path already
        // sees the snapshot version in `active_snapshots` (GC safety).
        // The increment is AcqRel so it is visible to the non-tx write
        // that does a Relaxed load — a conservative over-approximation
        // is fine (the footprint is cheap to write).
        self.active_serializable_count
            .fetch_add(1, Ordering::AcqRel);
        SnapshotGuard {
            registered,
            snapshots: Arc::clone(&self.active_snapshots),
            versions: Arc::clone(&self.snapshot_versions),
            #[cfg(test)]
            drop_hook: None,
            serializable_count: Some(Arc::clone(&self.active_serializable_count)),
        }
    }

    /// A10 TOCTOU fix: register-then-verify-then-reconcile, protected by
    /// an in-flight barrier.
    ///
    /// The in-flight barrier (`active_snapshots_opening`) is incremented
    /// BEFORE the first `completion().watermark()` read and decremented AFTER the
    /// final registration completes. While the counter is non-zero, vacuum's
    /// fast path defers ALL physical deletion — closing the race for an
    /// unbounded number of writer cycles (a stalled reader's floor version
    /// is never deleted, no matter how many writes happen while it is
    /// mid-registration).
    ///
    /// The barrier uses an RAII guard (`OpeningBarrier`) so the counter is
    /// correctly decremented even if the future is dropped/cancelled mid-
    /// registration — a leaked increment would permanently block vacuum's
    /// fast path.
    ///
    /// After registration:
    /// 1. Read the floor `v0 = completion().watermark()`.
    /// 2. Atomically bump the refcount for `v0` in `active_snapshots`.
    /// 3. Re-read the floor. If it advanced to `v1 > v0`, register `v1`
    ///    too (the stale `v0` entry is left for the guard's Drop to clean
    ///    up via refcount-decrement).
    ///
    /// Bounded retry: the floor only moves forward, so at most one
    /// reconciliation iteration is needed in practice.
    async fn register_snapshot(&self) -> Vec<u64> {
        // A10 barrier: increment BEFORE reading the floor. The AcqRel
        // store ensures vacuum's Acquire load sees the increment before
        // the reader's subsequent Acquire load of `completion().watermark()`.
        let _barrier = OpeningBarrier::new(&self.active_snapshots_opening);

        const MAX_RETRIES: usize = 4;
        let mut registered = Vec::with_capacity(2);
        let mut current = self.completion().watermark();
        loop {
            self.bump_refcount(current).await;
            registered.push(current);
            let next = self.completion().watermark();
            if next == current || registered.len() >= MAX_RETRIES {
                break;
            }
            // Floor moved forward — loop to register the new floor too.
            current = next;
        }
        registered
        // `_barrier` drops here: AcqRel decrement, visible to vacuum only
        // AFTER all registrations above are complete.
    }

    /// Atomically insert-or-bump the refcount for `version` in
    /// `active_snapshots`. Uses the per-entry exclusive lock held by
    /// `entry_sync` so the read-modify-write is race-free against
    /// concurrent openers and droppers.
    ///
    /// DEADLOCK FIX (same class as #589 / `cells` map, commit `7a4abf62`):
    /// `entry_sync`, NOT `entry_async`. This map is read on EVERY
    /// transaction end via the SYNCHRONOUS `SnapshotGuard::drop` →
    /// `entry_sync(v)` (`active_snapshots.entry_sync`), which PARKS the
    /// calling OS thread while the bucket is exclusively locked (scc's
    /// synchronous accessor parks the worker until the lock is free), and
    /// also scanned via the SYNCHRONOUS `min_alive` / `active_snapshots_empty`
    /// GC-tick paths (`iter_sync` / `is_empty`). `entry_async`'s wait path
    /// is lock-HANDOFF: on release, saa GRANTS the exclusive bucket lock to
    /// the suspended waiter TASK, which then holds it while sitting in
    /// tokio's run queue until next polled. This map is keyed by version,
    /// and EVERY concurrent `open_snapshot` targets the SAME key — the
    /// CURRENT `last_committed` version — so all contention funnels onto
    /// ONE bucket by construction. If, in the lock-handoff window, every
    /// runtime worker thread runs a `SnapshotGuard::drop` (every tx end)
    /// or a GC-tick `iter_sync` on that same bucket, all workers park and
    /// the lock-owning `bump_refcount` task can never be polled again →
    /// whole-runtime deadlock. With `entry_sync` the exclusive lock is
    /// only ever held by a RUNNING thread for a few instructions (a
    /// saturating increment), so every synchronous-reader wait is
    /// structurally bounded — the deadlock window is closed by
    /// construction, not papered over with a longer timeout. The fn stays
    /// `async` to keep its call sites / public shape unchanged; it no
    /// longer suspends.
    /// LOCK ORDER: the hash-map bucket entry lock is held across the
    /// tree-index update for the same version; the tree is NEVER touched
    /// for a version whose map entry is not currently locked — this makes
    /// map+tree mutations atomic per version and closes the drop/open
    /// handoff race.
    async fn bump_refcount(&self, version: u64) {
        match self.active_snapshots.entry_sync(version) {
            scc::hash_map::Entry::Occupied(mut e) => {
                *e.get_mut() = e.get().saturating_add(1);
            }
            scc::hash_map::Entry::Vacant(e) => {
                // LOCK ORDER: hold the newly-inserted entry's bucket lock
                // (via `occ`) across the tree-index insert for the same
                // version; the tree is NEVER touched for a version whose
                // map entry is not currently locked — map+tree mutations
                // are atomic per version, closing the drop/open handoff race.
                let occ = e.insert_entry(1u64);
                let _ = self.snapshot_versions.insert_sync(version, ());
                drop(occ);
            }
        }
    }

    /// Test-only: install a callback at the actual last-ref drop interlock.
    #[cfg(test)]
    pub(crate) fn test_set_drop_hook(
        &self,
        guard: &mut SnapshotGuard,
        hook: impl FnOnce(bool) + Send + 'static,
    ) {
        guard.drop_hook = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn snapshot_min_index_contains(&self, v: u64) -> bool {
        self.snapshot_versions.contains(&v)
    }

    /// Minimum GC floor: `min(active snapshots U {completion watermark})`.
    /// The contiguous completion watermark W is an always-live snapshot, even
    /// when no snapshots are registered or all explicit pins are above W.
    /// `last_committed` is eager and non-contiguous: pruning to it could lose
    /// history needed at W and the commit-log window `(W, last]` needed by SSI.
    ///
    /// A10 in-flight barrier awareness: while any `open_snapshot` call is
    /// mid-registration (`active_snapshots_opening > 0`), this function
    /// returns `0` — the maximally conservative floor. This means:
    /// * History GC (`gc_below`, `vacuum_key`, `purge_below_ts`): no version
    ///   is reclaimable (all versions `>= 0` are sacred).
    /// * `prune_commit_log_below`: no records are pruned (real commit
    ///   versions start at 1, so `commit_version <= 0` matches nothing).
    /// * `prune_version_cache`: no entries are evicted (all `version >= 0`).
    ///
    /// This protects the in-flight reader unconditionally: its floor is some
    /// `v_old <= last_committed()` (captured before the barrier increment),
    /// and since `min_alive() == 0 < v_old`, every version the reader might
    /// need is sacred. Once the reader completes registration, its version
    /// appears in `active_snapshots` and `min_alive()` returns the true
    /// minimum — GC resumes normally.
    pub fn min_alive(&self) -> u64 {
        // Fast path: no in-flight openers. This is the overwhelmingly common
        // case, so we check it FIRST to avoid the active_snapshots scan when
        // there is no contention.
        if self.active_snapshots_opening.load(Ordering::Acquire) > 0 {
            return 0;
        }
        let mut min = self.completion().watermark();
        // Bounded lookup: first key of the ordered distinct-version index is
        // the minimum pinned snapshot — no full-map iteration.
        let guard = scc::Guard::new();
        if let Some((k, _)) = self.snapshot_versions.range(0u64.., &guard).next() {
            #[cfg(test)]
            MIN_ALIVE_PROBE.with(|probe| probe.set(probe.get() + 1));
            min = min.min(*k);
        }
        min
    }

    /// True if no transaction has an open snapshot.
    ///
    /// A10 in-flight barrier awareness: returns `false` (i.e. "snapshots
    /// ARE active") while any `open_snapshot` call is mid-registration,
    /// even if the registration hasn't landed in `active_snapshots` yet.
    /// This ensures callers that use `active_snapshots_empty()` to decide
    /// whether to keep an anchor (e.g. `vacuum_key`'s scan path at
    /// `have_live_snapshot = !active_snapshots_empty()`) behave
    /// conservatively during the registration window.
    pub fn active_snapshots_empty(&self) -> bool {
        self.active_snapshots.is_empty() && !self.snapshots_opening()
    }

    /// A10 in-flight barrier: true if at least one `open_snapshot` call has
    /// begun (incremented the counter) but not yet completed registration in
    /// `active_snapshots`. Vacuum's fast path checks this to defer ALL
    /// physical deletion while a reader is mid-registration.
    pub fn snapshots_opening(&self) -> bool {
        self.active_snapshots_opening.load(Ordering::Acquire) > 0
    }

    /// Test-only: simulate a stalled `open_snapshot` caller that has
    /// incremented the in-flight barrier but not yet completed registration.
    /// Returns an RAII guard that decrements on drop. This lets tests
    /// deterministically reproduce the multi-generation stall scenario
    /// (reader captured floor, held in-flight across N writes, then
    /// completes) without relying on scheduler timing.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub(crate) fn test_hold_opening_barrier(&self) -> OpeningBarrier<'_> {
        OpeningBarrier::new(&self.active_snapshots_opening)
    }
}
