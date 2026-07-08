use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};

use super::key_lock::{Holder, KeyLock, LockMode};
use super::MvccStore;

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
        key: Bytes,
        tx_version: u64,
        wounded: Arc<AtomicBool>,
        wound_notify: Arc<tokio::sync::Notify>,
        mode: LockMode,
    ) -> DbResult<()> {
        // Get-or-insert the KeyLock for this key. The Arc is shared between
        // concurrent requesters so they coordinate on the same Mutex/Notify.
        let lock = match self.locks.entry_async(key).await {
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
    pub async fn release_locks(&self, tx_version: u64, keys: &[Bytes]) {
        for key in keys {
            let Some(lock) = self.locks.get(key).map(|e| Arc::clone(e.get())) else {
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
