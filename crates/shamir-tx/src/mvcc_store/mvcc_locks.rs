use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::RecordKey;

use super::key_lock::{Holder, KeyLock, LockMode};
use super::MvccStore;
use super::RecordCell;

impl MvccStore {
    /// Acquire a Level-3 pessimistic lock on `key` for tx `tx_version` in
    /// `mode`, using the wound-wait protocol.
    ///
    /// `wounded` is the requesting tx's shared abort flag. If a strictly
    /// older (higher-priority) tx wounds THIS tx while it is waiting, the
    /// flag is set and this call returns
    /// [`DbError::Conflict`](shamir_storage::error::DbError::Conflict) so the
    /// tx aborts instead of acquiring the lock.
    ///
    /// Algorithm (loop):
    /// 1. Lock `state`.
    /// 2. If the requested `mode` is compatible with the current holders
    ///    (Shared+Shared compatible; anything with Exclusive incompatible;
    ///    a holder with the SAME `tx_version` is compatible — re-entrant —
    ///    EXCEPT a Shared→Exclusive UPGRADE when OTHER txs also hold the
    ///    key, which is treated as incompatible and falls through to step 3
    ///    so the conflicting other holders are resolved via wound-wait;
    ///    audit A6), add the holder, set `mode`, return `Ok(())`.
    /// 3. Otherwise, for every CONFLICTING holder `H`:
    ///    - `tx_version < H.tx_version` (requester OLDER / higher priority):
    ///      WOUND `H` — set `H.wounded`, remove `H` from holders. After
    ///      wounding all conflicting younger holders, `notify_waiters()` and
    ///      loop again (the requester may now fit).
    ///    - `tx_version > H.tx_version` (requester YOUNGER): the requester
    ///      must WAIT. Drop the state lock, await `notify.notified()`, loop.
    ///    - `tx_version == H.tx_version`: same tx — compatible, skip.
    /// 4. Before waiting AND after being woken, check `wounded.load()`: if
    ///    this tx was wounded while waiting, return the conflict error.
    ///
    /// Correctness: a requester only ever waits on strictly-older holders
    /// and only ever wounds strictly-younger ones, so the wait-for graph
    /// respects the total version order and cannot cycle (deadlock-free by
    /// construction — no detector needed).
    pub async fn lock_key(
        &self,
        key: RecordKey,
        tx_version: u64,
        wounded: Arc<AtomicBool>,
        wound_notify: Arc<tokio::sync::Notify>,
        mode: LockMode,
    ) -> DbResult<()> {
        // Get-or-insert the KeyLock for this key. The Arc is shared between
        // concurrent requesters so they coordinate on the same Mutex/Notify.
        //
        // DEADLOCK FIX (same class as #589 / `cells` map, commit `7a4abf62`):
        // `entry_sync`, NOT `entry_async`. The `locks` map is read on EVERY
        // Level-3 commit AND abort via the SYNCHRONOUS `release_locks` →
        // `self.locks.get_sync(key)` (`get_sync` takes a shared bucket lock
        // and PARKS the calling OS thread while the bucket is exclusively
        // owned). `entry_async`'s wait path is lock-HANDOFF: on release,
        // saa GRANTS the exclusive bucket lock to the suspended waiter
        // TASK, which then holds it while sitting in tokio's run queue
        // until next polled. The pessimistic-locking hot-key case — many
        // txs contending for the SAME `RecordKey` — funnels onto ONE
        // bucket by construction. If, in the lock-handoff window, every
        // runtime worker thread runs `release_locks` → `get_sync(k)` on
        // that same bucket, all workers park and the lock-owning
        // `lock_key` task can never be polled again → whole-runtime
        // deadlock. With `entry_sync` the exclusive lock is only ever
        // held by a RUNNING thread for a few instructions (an
        // `Arc::clone`-or-insert), so every `get_sync` wait is bounded.
        // The fn stays `async` to keep its call sites unchanged; this
        // particular call no longer suspends (the rest of the fn still
        // legitimately awaits the per-key `state` Mutex / Notify).
        let lock = match self.locks.entry_sync(key) {
            scc::hash_map::Entry::Occupied(e) => Arc::clone(e.get()),
            scc::hash_map::Entry::Vacant(e) => {
                let arc = Arc::new(KeyLock::new());
                e.insert_entry(Arc::clone(&arc));
                arc
            }
        };

        loop {
            // (4) Abort early if this tx was already wounded by an older tx.
            if wounded.load(Ordering::Acquire) {
                return Err(DbError::Conflict(format!(
                    "tx {} wounded (wound-wait abort) before acquiring lock",
                    tx_version
                )));
            }

            let mut state = lock.state.lock().await;

            // (2) Compatibility check.
            //
            // - Re-entrant (this tx already holds the key): compatible
            //   EXCEPT for a Shared→Exclusive UPGRADE when OTHER txs also
            //   hold the key (audit A6). A re-entrant Exclusive upgrade
            //   with other Shared holders present would violate the
            //   "Exclusive ⇒ exactly one holder" invariant, so it must
            //   fall through to the wound-wait partition logic below
            //   (wound younger others / wait for older others) instead of
            //   being blindly granted. A re-entrant Shared re-acquire, a
            //   re-entrant same-mode re-acquire, or a re-entrant Exclusive
            //   upgrade with NO other holders (solo upgrade) is always
            //   safe and stays on the instant-grant fast path.
            // - Shared request vs Shared holders: compatible (multiple readers).
            // - Anything else (Exclusive involved, or Shared vs Exclusive):
            //   incompatible.
            let re_entrant = state.held_by(tx_version);
            let has_other_holders = state.holders.iter().any(|h| h.tx_version != tx_version);
            let re_entrant_compatible =
                re_entrant && (mode != LockMode::Exclusive || !has_other_holders);
            let compatible = re_entrant_compatible
                || state.mode.is_none()
                || (mode == LockMode::Shared && state.mode == Some(LockMode::Shared));

            if compatible {
                // Re-entrant re-acquire: if this tx already holds the key,
                // do NOT push a duplicate holder (would violate the
                // distinct-id invariant and skew mode recomputation). Just
                // return Ok — the existing holder already grants access.
                if !re_entrant {
                    state.holders.push(Holder {
                        tx_version,
                        wounded: Arc::clone(&wounded),
                        wound_notify: Arc::clone(&wound_notify),
                    });
                }
                // Set the mode. An Exclusive requester that re-enters a key
                // it already holds Shared upgrades the recorded mode so a
                // later third-tx Shared requester correctly sees conflict.
                state.mode = Some(mode);
                return Ok(());
            }

            // (3) Incompatible. Partition the conflicting holders.
            //
            // Younger holders (tx_version < H.tx_version) get WOUNDED and
            // removed. If ANY holder is strictly OLDER than the requester
            // (tx_version > H.tx_version) and conflicts, the requester must
            // WAIT (it cannot wound the older holder). Same-tx holders are
            // never conflicting (handled by the re-entrant branch above).
            let mut must_wait = false;
            let mut wounded_any = false;
            // Collect indices of younger holders to remove (wound them).
            // Iterate back-to-front so swap_remove preserves indices.
            let mut i = state.holders.len();
            while i > 0 {
                i -= 1;
                let h = &state.holders[i];
                // Skip same-tx holders (re-entrant, never conflicting).
                if h.tx_version == tx_version {
                    continue;
                }
                // This holder conflicts with the request (we're in the
                // incompatible branch). Decide wound vs wait by age.
                if tx_version < h.tx_version {
                    // Requester is OLDER → wound the younger holder. Set
                    // the flag AND wake the holder's per-tx notify so it
                    // observes the wound even if it is parked waiting on
                    // a DIFFERENT key (load-bearing for deadlock-freedom:
                    // a wound on key Y must wake a tx parked on key X).
                    h.wounded.store(true, Ordering::Release);
                    h.wound_notify.notify_one();
                    wounded_any = true;
                    state.holders.swap_remove(i);
                } else {
                    // Requester is YOUNGER → must wait for the older holder.
                    must_wait = true;
                }
            }

            if wounded_any {
                // Recompute the aggregate mode from surviving holders.
                state.recompute_mode();
                // Wake any waiters so they observe the wounds / freed slots.
                lock.notify.notify_waiters();
            }

            if must_wait {
                // (4) Re-check wounded before suspending — an older tx may
                // have wounded this one between the top-of-loop check and
                // here (we held the state lock the whole time, so in fact
                // only wounds issued before we acquired the lock could have
                // landed; still, the check is cheap and correct).
                if wounded.load(Ordering::Acquire) {
                    return Err(DbError::Conflict(format!(
                        "tx {} wounded (wound-wait abort) while waiting",
                        tx_version
                    )));
                }
                // Register the key-notify waiter BEFORE dropping the state
                // lock. `tokio::sync::Notify::notify_waiters` only wakes
                // futures already in the notified() queue — it does NOT store
                // a permit. If we created the future after `drop(state)`, a
                // `release_locks` → `notify_waiters()` firing in the window
                // between the drop and the first poll would be LOST, and the
                // waiting tx could hang forever on a multi-threaded runtime.
                // `enable()` enters the waiter queue synchronously while we
                // still hold `state`, closing that window.
                let mut notified = Box::pin(lock.notify.notified());
                notified.as_mut().enable();
                drop(state);
                tokio::select! {
                    _ = notified.as_mut() => {}
                    _ = wound_notify.notified() => {}
                }
                continue;
            }

            // We wounded everyone conflicting and nobody older remains. Loop
            // to re-acquire the state lock and retry the compatibility check
            // (the freed slots should now let us in).
        }
    }

    /// Release every lock held by `tx_version` on the given `keys`.
    ///
    /// Called on BOTH commit and abort/drop of a Level-3 tx. For each key,
    /// locks the state, removes all holders with the given `tx_version`,
    /// recomputes the mode, and wakes waiters. Leftover empty entries are
    /// kept in the map (cheap; GC is intentionally not done here).
    pub async fn release_locks(&self, tx_version: u64, keys: &[RecordKey]) {
        for key in keys {
            let Some(lock) = self.locks.get_sync(key).map(|e| Arc::clone(e.get())) else {
                continue;
            };
            let mut state = lock.state.lock().await;
            let before = state.holders.len();
            state.holders.retain(|h| h.tx_version != tx_version);
            if state.holders.len() != before {
                state.recompute_mode();
                // Wake waiters so a blocked tx can re-evaluate.
                lock.notify.notify_waiters();
            }
        }
    }
}

impl MvccStore {
    /// Publish `version` into the key's cell. Atomic modify-or-insert;
    /// advances the cell's version to `version` on every write path.
    /// (Bump-first ordering is the CALLER's job — this only performs the
    /// cell mutation.)
    ///
    /// A2 — max-monotonic: on the OCCUPIED branch the cell's `version` is
    /// advanced ONLY when `version` is strictly greater than the cell's
    /// current value. This prevents a slow drainer / recovery replay (which
    /// seeds the cell from a durable write that may be older than an
    /// in-memory commit that raced ahead during the drainer's `.await`
    /// suspension) from regressing the cell backward and causing stale reads
    /// / masked SSI conflicts. The VACANT branch is untouched (a brand-new
    /// cell always seeds at the offered version). The NORMAL non-racing
    /// drain path always offers `<=` the current version, so this guard is a
    /// no-op there.
    ///
    /// DEADLOCK FIX (#589 root cause): `entry_sync`, NOT `entry_async`. The
    /// cells map is read on every hot read path via the SYNCHRONOUS
    /// `read_sync` (`current_version` / `version_of` / `live_version`), which
    /// PARKS the calling OS thread while the bucket is exclusively locked
    /// (scc's `Reader::lock_sync` → saa `share_sync`). `entry_async`'s wait
    /// path is lock-HANDOFF: on release, saa GRANTS the exclusive lock to the
    /// suspended waiter task, which then holds it while sitting in tokio's
    /// run queue until next polled. If, in that window, every runtime worker
    /// thread enters `read_sync` on the same (hot-key) bucket, all workers
    /// park, the lock-owning task can never be polled again, and the whole
    /// runtime deadlocks — reproduced deterministically by the D2
    /// overlay-ordering history-arm test under CPU contention (CI timeout;
    /// see `tests/mvcc_store_tests/overlay_ordering_tests.rs`). With
    /// `entry_sync` the exclusive lock is only ever held by a RUNNING thread
    /// for a few instructions (same contract as `finalize_reservation` /
    /// `try_reserve` on this same map), so every `read_sync` wait is bounded.
    /// The fn stays `async` to keep its many call sites / public shape
    /// unchanged; it no longer suspends.
    ///
    /// The body lives in `write_receipt::cell_publish`.
    pub(crate) async fn publish_cell(&self, key: RecordKey, version: u64) {
        self.publish_cell_sync(key, version);
    }

    /// A10 vacuum anchor deferral: atomically set the cell's `vacuum_anchor`
    /// to `new_anchor` and return the PREVIOUS value. Uses the sync `entry`
    /// API (per-entry exclusive lock) so the read-and-swap is race-free
    /// against concurrent vacuum calls on the same key. If the cell is
    /// absent (cold-start or evicted), returns `None` and does nothing —
    /// the deferred-anchor optimisation only applies when the cell is
    /// resident.
    pub(crate) fn swap_vacuum_anchor(&self, key: &[u8], new_anchor: u64) -> Option<u64> {
        // Inline-cheap for the 16-byte `RecordId` shape; builds the cell map's
        // `RecordKey` entry key without the prior `Bytes` heap copy.
        match self.cells.entry_sync(RecordKey::from_slice(key)) {
            scc::hash_map::Entry::Occupied(mut e) => {
                let cell = e.get_mut();
                let prev = cell.vacuum_anchor;
                cell.vacuum_anchor = new_anchor;
                Some(prev)
            }
            scc::hash_map::Entry::Vacant(_) => None,
        }
    }

    // ========================================================================
    // SSI fix S1 — cell-reservation primitive (additive; NOT yet wired).
    //
    // Three atomic acts on the per-key `RecordCell`, all via `cells.entry(key)`
    // (scc per-entry exclusive — the same check-and-update primitive
    // `publish_cell_sync` already relies on). They turn "publish decides who
    // won" (after the WAL commit point) into "claim decides who won" (BEFORE
    // it): a single committer claims the cell, losers see the claim and abort
    // with `SsiConflict` having never touched the WAL.
    //
    // S1 only BUILDS these — `try_reserve` is never called on the live commit
    // path yet, so `reserved_by` stays `0` everywhere and behaviour is
    // byte-identical. S2 invokes `try_reserve` in `pre_commit` (after
    // read-validate, before WAL), `finalize_reservation` in the publish path,
    // and `release_reservation` from the abort-path RAII guard.
    //
    // Sync (`entry`, not `entry_async`) because every call site is the
    // synchronous commit hot path — same rationale as `publish_cell_sync`:
    // the cell map is lock-free / sharded, so `entry` is a bounded CAS, not a
    // contended lock.
    // ========================================================================

    /// SSI fix S1 — atomically CLAIM this key's cell for `txn_id`, the explicit
    /// serialization point for a write-write conflict.
    ///
    /// Returns `true` iff the claim WON (this committer may proceed to WAL /
    /// publish); `false` on CONFLICT (another committer holds the claim, or the
    /// cell has already advanced past this committer's snapshot — a stale
    /// write). A conflict NEVER blocks — the caller aborts with `SsiConflict`.
    ///
    /// Cases, all inside one atomic `entry`:
    /// - **Vacant** → the key has never published a version, so it is free.
    ///   Insert `RecordCell { version: 0, reserved_by: txn_id }` and WIN. The
    ///   `version: 0` sentinel matches `current_version`'s "no version tracked"
    ///   convention; `finalize_reservation` later overwrites it with the real
    ///   commit version.
    /// - **Occupied** → WIN iff `version <= snapshot_version && reserved_by == 0`:
    ///   the cell is unclaimed AND has not advanced past our snapshot. Set
    ///   `reserved_by = txn_id`. Otherwise CONFLICT (`false`): either the cell
    ///   moved past the snapshot (someone published — stale-write detection) or
    ///   it is already claimed by another committer.
    ///
    /// S2 wires this into the engine's `pre_commit` (after read-validate,
    /// before WAL): `pub` (not `pub(crate)`) so the cross-crate `shamir-engine`
    /// commit path can call it to claim each write-set key. The claim is the
    /// explicit serialization point that makes "exactly one committer wins" hold
    /// for non-unique tables under true parallelism.
    pub fn try_reserve(&self, key: RecordKey, snapshot_version: u64, txn_id: u64) -> bool {
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                let cell = e.get_mut();
                if cell.version <= snapshot_version && cell.reserved_by == 0 {
                    cell.reserved_by = txn_id;
                    true
                } else {
                    false
                }
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version: 0,
                    vacuum_anchor: 0,
                    reserved_by: txn_id,
                });
                true
            }
        }
    }

    /// SSI fix S1 — atomically convert a held claim into the published
    /// `version` and CLEAR the reservation, in one `entry`.
    ///
    /// Called on the publish path (S2: Phase 5a) AFTER the committer has won
    /// its claim and the write is becoming visible. Sets
    /// `version = max(current_version, version)` and `reserved_by = 0`.
    /// Invariant: finalizing a lower or equal version must never regress the
    /// cell's version, even when materialization completes out of order;
    /// reservation clearing still happens unconditionally under `entry_sync`.
    /// If the cell is somehow Vacant (defensive — a claim should have inserted
    /// it), insert `RecordCell { version,
    /// reserved_by: 0 }` so the published version is never lost.
    ///
    /// S2 wires this into the publish path (`apply_committed_visible`,
    /// Phase 5a) in place of the prior `publish_cell_sync` — it is a strict
    /// superset (sets `version` AND clears `reserved_by`).
    pub(crate) fn finalize_reservation(&self, key: RecordKey, version: u64) {
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                if version > e.get().version {
                    e.get_mut().version = version;
                }
                e.get_mut().reserved_by = 0;
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version,
                    vacuum_anchor: 0,
                    reserved_by: 0,
                });
            }
        }
    }

    /// SSI fix S1 — release a claim held by `txn_id` (abort path), in one
    /// `entry`. Idempotent and ownership-checked: only clears `reserved_by`
    /// when it still equals `txn_id`.
    ///
    /// A no-op when the cell is Vacant, or when `reserved_by != txn_id` — the
    /// latter covers both "another committer now owns the claim" (we must not
    /// steal it) and "our claim was already finalized" (`reserved_by` is
    /// already `0`). This makes the RAII guard's `Drop` safe to fire after a
    /// successful `finalize_reservation`.
    pub(crate) fn release_reservation(&self, key: RecordKey, txn_id: u64) {
        if let scc::hash_map::Entry::Occupied(mut e) = self.cells.entry_sync(key) {
            let cell = e.get_mut();
            if cell.reserved_by == txn_id {
                cell.reserved_by = 0;
            }
        }
    }

    // ========================================================================
    // Level-3 pessimistic locking (wound-wait, deadlock-free).
    //
    // These methods are ONLY called from `IsolationLevel::Pessimistic` code
    // paths (see `table_manager` / `write_exec`). The snapshot / serializable
    // read/write paths never touch `locks`, so when no Level-3 tx runs the
    // registry stays empty and there is zero overhead on the hot paths.
    // ========================================================================

    /// Number of keys currently holding a Level-3 lock entry. Used by tests
    /// to assert the zero-overhead invariant (snapshot/serializable txs never
    /// populate `locks`).
    #[allow(clippy::disallowed_methods)] // O(N) ack: telemetry/test accessor, off hot path
    pub fn locks_len(&self) -> usize {
        self.locks.len()
    }
}
