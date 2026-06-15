//! RAII guard for SSI cell-reservations (SSI fix S1).
//!
//! A [`CellReservationGuard`] owns the obligation to RELEASE every cell a
//! committer has claimed via [`MvccStore::try_reserve`](crate::MvccStore::try_reserve)
//! if the commit does not reach its publish point. It is the abort-path twin
//! of [`finalize_reservation`](crate::MvccStore::finalize_reservation): on a
//! successful commit the publisher calls `finalize_reservation` for each key
//! (which clears `reserved_by`) and then [`disarm`](CellReservationGuard::disarm)s
//! the guard so its `Drop` is a no-op; on ANY early return / `?`-propagated
//! error / panic before that point the still-`armed` `Drop` calls
//! [`release_reservation`](crate::MvccStore::release_reservation) for every
//! claimed key, so a burned commit never strands a claim that would wedge
//! every competing writer of those keys.
//!
//! Mirrors [`VersionGuard`](crate::VersionGuard): it holds an `Arc` to the
//! shared state it must touch on `Drop` (here the [`MvccStore`]) — never a
//! back-reference to a higher-level object — so its `Drop` is self-contained
//! and synchronous (`release_reservation` is a `scc` `entry` CAS, never
//! `async`), which makes the `Drop` sound.
//!
//! S1 builds and unit-tests this guard standalone; it is NOT yet wired into
//! the commit path (that is S2, where it composes with `VersionGuard` — abort
//! releases both). In S2 the write-set is claimed in `pre_commit` (after
//! read-validate, before WAL), each won key is `add`ed to the guard, and the
//! guard is `disarm`ed once the publisher has finalized every claim.

use std::sync::Arc;

use bytes::Bytes;

use crate::mvcc_store::MvccStore;

/// RAII owner of a committer's SSI cell-reservations.
///
/// Holds an `Arc<MvccStore>` (the map the claims live in), the claimant's
/// `txn_id` (the owner marker `release_reservation` checks against), and the
/// list of keys claimed so far. While `armed`, `Drop` releases every claimed
/// key for `txn_id`; [`disarm`](Self::disarm) clears `armed` after a
/// successful publish so `Drop` does no redundant lookups.
#[must_use = "a CellReservationGuard must be disarmed after the claims are \
              finalized, else it releases every held reservation on drop"]
pub struct CellReservationGuard {
    store: Arc<MvccStore>,
    txn_id: u64,
    /// Keys claimed by this committer (each a won [`MvccStore::try_reserve`]).
    /// `Drop` releases exactly these.
    keys: Vec<Bytes>,
    armed: bool,
}

impl CellReservationGuard {
    /// Construct an empty, armed guard for `txn_id` over `store`.
    ///
    /// No claim is taken here — the caller claims via
    /// [`MvccStore::try_reserve`] and records each won key with
    /// [`add`](Self::add).
    pub fn new(store: Arc<MvccStore>, txn_id: u64) -> Self {
        Self {
            store,
            txn_id,
            keys: Vec::new(),
            armed: true,
        }
    }

    /// Record `key` as claimed by this committer — call after a `try_reserve`
    /// for `key` returned `true`. On abort (`Drop` while armed) the claim on
    /// `key` is released.
    pub fn add(&mut self, key: Bytes) {
        self.keys.push(key);
    }

    /// The `txn_id` this guard releases reservations for.
    pub fn txn_id(&self) -> u64 {
        self.txn_id
    }

    /// Number of keys this guard would release on drop. Test / telemetry
    /// accessor.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether this guard holds no claimed keys.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Disarm the guard: the commit succeeded and the publisher has already
    /// finalized every claim (`finalize_reservation` cleared each
    /// `reserved_by`), so `Drop` must NOT run — there is nothing to release
    /// and the lookups would be pure waste.
    ///
    /// Idempotent. Takes `&mut self` (not `self`) so it can be called from a
    /// commit path that keeps the guard alive for the rest of the scope.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CellReservationGuard {
    fn drop(&mut self) {
        if self.armed {
            // Abort path: no `disarm()` reached this scope, so the commit did
            // not finish finalizing its claims. Release every claim this
            // committer holds. `release_reservation` is ownership-checked and
            // idempotent: a key already finalized (or re-claimed by another
            // committer) is left untouched.
            for key in self.keys.drain(..) {
                self.store.release_reservation(key, self.txn_id);
            }
        }
    }
}
