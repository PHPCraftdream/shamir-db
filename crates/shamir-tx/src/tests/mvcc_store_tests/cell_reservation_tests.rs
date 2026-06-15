//! SSI fix S1 — unit tests for the cell-reservation primitive
//! (`try_reserve` / `finalize_reservation` / `release_reservation` on
//! [`MvccStore`]) and its RAII [`CellReservationGuard`].
//!
//! These prove the primitive in isolation; it is NOT wired into the commit
//! path in S1. The keystone is `concurrent_try_reserve_exactly_one_wins`,
//! which asserts the atomicity claim (per-entry scc exclusive) that the whole
//! fix rests on: under N racing claimants of ONE cell, EXACTLY one wins —
//! independent of the scheduler.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;

use crate::cell_reservation_guard::CellReservationGuard;
use crate::mvcc_store::MvccStore;

use super::helpers::make_mvcc;

/// Read `(version, reserved_by)` straight out of the cell for assertions.
/// Returns `None` when the key has no cell.
fn cell_state(mvcc: &MvccStore, key: &[u8]) -> Option<(u64, u64)> {
    // `current_version` only exposes `version`; the reservation marker lives
    // on `RecordCell`, so probe the map directly (the live path never reads
    // the marker — asserting it is the whole point here). Other mvcc_store
    // tests probe `mvcc.cells` the same way (see `version_tests.rs`).
    mvcc.cells.read(key, |_, c| (c.version, c.reserved_by))
}

#[tokio::test]
async fn try_reserve_wins_on_free_cell() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // Vacant cell — never published. A claim against any snapshot wins and
    // inserts a `version: 0` cell owned by the claimant.
    assert!(
        mvcc.try_reserve(key.clone(), 0, 7),
        "vacant cell must be claimable"
    );
    assert_eq!(cell_state(&mvcc, &key), Some((0, 7)));
}

#[tokio::test]
async fn try_reserve_wins_on_published_unclaimed_cell_within_snapshot() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // Publish version 5 (Occupied, reserved_by == 0).
    mvcc.publish_cell(key.clone(), 5).await;
    // Snapshot at 10 >= 5 and unclaimed → win.
    assert!(mvcc.try_reserve(key.clone(), 10, 3));
    assert_eq!(cell_state(&mvcc, &key), Some((5, 3)));
}

#[tokio::test]
async fn try_reserve_conflicts_when_already_reserved() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");
    mvcc.publish_cell(key.clone(), 1).await;

    // First claimant wins.
    assert!(mvcc.try_reserve(key.clone(), 5, 100));
    // Second claimant on the SAME (claimed) cell loses — no block, no steal.
    assert!(!mvcc.try_reserve(key.clone(), 5, 200));
    // Still owned by the first claimant, version untouched.
    assert_eq!(cell_state(&mvcc, &key), Some((1, 100)));
}

#[tokio::test]
async fn try_reserve_conflicts_on_stale_snapshot() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // Cell has advanced to version 9 (someone published since our snapshot).
    mvcc.publish_cell(key.clone(), 9).await;
    // Our snapshot is 4 < 9 → stale-write detection → conflict, even though
    // the cell is unclaimed (`reserved_by == 0`).
    assert!(!mvcc.try_reserve(key.clone(), 4, 55));
    // Unchanged: not claimed.
    assert_eq!(cell_state(&mvcc, &key), Some((9, 0)));
}

#[tokio::test]
async fn finalize_sets_version_and_clears_reservation() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // Claim a vacant cell, then finalize to commit-version 42.
    assert!(mvcc.try_reserve(key.clone(), 0, 9));
    assert_eq!(cell_state(&mvcc, &key), Some((0, 9)));

    mvcc.finalize_reservation(key.clone(), 42);
    assert_eq!(
        cell_state(&mvcc, &key),
        Some((42, 0)),
        "finalize publishes the version and clears the claim"
    );
}

#[tokio::test]
async fn finalize_on_vacant_cell_inserts_published_version() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // Defensive path: finalize a never-claimed, never-published key.
    mvcc.finalize_reservation(key.clone(), 7);
    assert_eq!(cell_state(&mvcc, &key), Some((7, 0)));
}

#[tokio::test]
async fn release_only_clears_own_reservation() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");
    mvcc.publish_cell(key.clone(), 2).await;
    assert!(mvcc.try_reserve(key.clone(), 5, 100));

    // A foreign txn_id must NOT clear our claim.
    mvcc.release_reservation(key.clone(), 999);
    assert_eq!(
        cell_state(&mvcc, &key),
        Some((2, 100)),
        "foreign release is a no-op"
    );

    // The owner releases it.
    mvcc.release_reservation(key.clone(), 100);
    assert_eq!(
        cell_state(&mvcc, &key),
        Some((2, 0)),
        "owner release clears the claim"
    );

    // Idempotent: releasing again (now reserved_by == 0) is a no-op.
    mvcc.release_reservation(key.clone(), 100);
    assert_eq!(cell_state(&mvcc, &key), Some((2, 0)));
}

#[tokio::test]
async fn release_after_finalize_is_noop() {
    // Models the success path: finalize cleared the claim; the guard's Drop
    // then releases — must be a no-op and must NOT clobber the published
    // version.
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");
    assert!(mvcc.try_reserve(key.clone(), 0, 11));
    mvcc.finalize_reservation(key.clone(), 30);

    mvcc.release_reservation(key.clone(), 11);
    assert_eq!(cell_state(&mvcc, &key), Some((30, 0)));
}

#[tokio::test]
async fn guard_drop_releases_held() {
    let mvcc = Arc::new(make_mvcc());
    let keys: Vec<Bytes> = (0..5u8).map(|i| Bytes::from(vec![i])).collect();

    {
        let mut guard = CellReservationGuard::new(Arc::clone(&mvcc), 77);
        for k in &keys {
            assert!(mvcc.try_reserve(k.clone(), 0, 77));
            guard.add(k.clone());
        }
        assert_eq!(guard.len(), 5);
        // Every cell is claimed by txn 77 while the guard is live.
        for k in &keys {
            assert_eq!(cell_state(&mvcc, k), Some((0, 77)));
        }
        // guard drops here (armed) → release all.
    }

    for k in &keys {
        assert_eq!(
            cell_state(&mvcc, k),
            Some((0, 0)),
            "armed Drop releases every held reservation"
        );
    }
}

#[tokio::test]
async fn guard_disarm_skips_release() {
    let mvcc = Arc::new(make_mvcc());
    let key = Bytes::from_static(b"k");

    {
        let mut guard = CellReservationGuard::new(Arc::clone(&mvcc), 8);
        assert!(mvcc.try_reserve(key.clone(), 0, 8));
        guard.add(key.clone());
        // Simulate the success path: publisher finalized the claim, then we
        // disarm so Drop is a no-op.
        mvcc.finalize_reservation(key.clone(), 12);
        guard.disarm();
        // guard drops here (disarmed) → no release.
    }

    // The finalized version stands; Drop did not run release.
    assert_eq!(cell_state(&mvcc, &key), Some((12, 0)));
}

/// KEYSTONE — N tasks race to `try_reserve` ONE cell; EXACTLY one wins each
/// round, regardless of the scheduler. This is the atomicity proof the whole
/// SSI fix depends on: `cells.entry(key)` is per-entry exclusive, so the
/// check-and-claim is one indivisible act and the "exactly one wins"
/// invariant holds under real parallelism.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_try_reserve_exactly_one_wins() {
    const ROUNDS: usize = 40;
    const CLAIMANTS: usize = 16;

    let mvcc = Arc::new(make_mvcc());

    for round in 0..ROUNDS {
        // Fresh key per round so each round starts from a Vacant cell — the
        // claim race is on a single never-before-seen cell.
        let key = Bytes::from(format!("contended-{round}").into_bytes());
        let wins = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::with_capacity(CLAIMANTS);
        for c in 0..CLAIMANTS {
            let mvcc = Arc::clone(&mvcc);
            let key = key.clone();
            let wins = Arc::clone(&wins);
            // Distinct, monotone txn_ids (round*CLAIMANTS + c + 1, never 0).
            let txn_id = (round * CLAIMANTS + c + 1) as u64;
            handles.push(tokio::spawn(async move {
                // snapshot_version u64::MAX so the ONLY arbiter is the
                // reservation marker, not stale-write detection — every
                // claimant is "fresh enough", so exactly one must win on the
                // `reserved_by == 0` check alone.
                if mvcc.try_reserve(key, u64::MAX, txn_id) {
                    wins.fetch_add(1, Ordering::AcqRel);
                }
            }));
        }
        for h in handles {
            h.await.expect("claimant task must not panic");
        }

        assert_eq!(
            wins.load(Ordering::Acquire),
            1,
            "round {round}: expected exactly one winner among {CLAIMANTS} claimants"
        );
    }
}
