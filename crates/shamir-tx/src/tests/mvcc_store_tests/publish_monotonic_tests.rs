//! A2 regression guard — max-monotonic `publish_cell` / `seed_version`.
//!
//! Audit finding A2 (`docs/audits/2026-07-06-concurrency-engine.md`): both
//! `publish_cell` (the per-op drain/recovery seed path) and `seed_version`
//! (the cold-read cache-seed path) unconditionally overwrote a cell's
//! `version`. When a slow drainer or recovery replay races a NEWER in-memory
//! commit, the delayed seed-from-durable-write would regress the cell's
//! version backward, causing stale reads and masking SSI write-write
//! conflicts.
//!
//! The fix is a strict-greater-than guard: a write only advances
//! `cell.version` when the offered version is strictly greater than the
//! cell's current version. These tests reproduce the exact interleaving
//! described in A2 directly against `publish_cell` and `seed_version`
//! (calling them in the "wrong" order — newer commit first, then the stale
//! delayed drain/recovery seed) and assert the cell does NOT regress.

use bytes::Bytes;

use crate::mvcc_store::MvccStore;

use super::helpers::make_mvcc;
use shamir_storage::types::RecordKey;

/// Read `(version, reserved_by)` straight out of the cell for assertions.
/// Returns `None` when the key has no cell (mirrors the probe pattern in
/// `cell_reservation_tests.rs`).
fn cell_state(mvcc: &MvccStore, key: &[u8]) -> Option<(u64, u64)> {
    mvcc.cells.read_sync(key, |_, c| (c.version, c.reserved_by))
}

// ----------------------------------------------------------------
// publish_cell
// ----------------------------------------------------------------

/// A2 core interleaving for `publish_cell`: the delayed drain/recovery
/// seed (an OLDER version) MUST NOT regress a cell that a newer commit
/// already advanced. Before the fix, the unconditional `e.get_mut().version
/// = version` overwrote 11 back down to 10.
#[tokio::test(flavor = "multi_thread")]
async fn publish_cell_never_regresses_version() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // 1. Transaction B commits k at version 11 (the newer in-memory commit
    //    wins the cell via Phase 5a / finalize_reservation -> publish_cell).
    mvcc.publish_cell(RecordKey::from(key.clone()), 11).await;
    assert_eq!(cell_state(&mvcc, &key), Some((11, 0)));

    // 2. The suspended drainer now resumes and finishes its STALE
    //    seed-from-durable-write for transaction A's version 10. This MUST
    //    NOT overwrite the cell's version back down to 10.
    mvcc.publish_cell(RecordKey::from(key.clone()), 10).await;

    assert_eq!(
        cell_state(&mvcc, &key),
        Some((11, 0)),
        "A2: publish_cell must be max-monotonic — a stale drain/recovery \
         seed must not regress the cell's version"
    );
    // The public SSI read-set accessor agrees with the raw cell probe.
    assert_eq!(mvcc.version_of(&key), 11);
}

/// The max-monotonic guard still lets a strictly-greater version advance
/// the cell (the normal forward-progress case). Guards against an
/// over-restrictive fix that would freeze the cell at its first value.
#[tokio::test(flavor = "multi_thread")]
async fn publish_cell_still_advances_on_greater_version() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    mvcc.publish_cell(RecordKey::from(key.clone()), 5).await;
    assert_eq!(cell_state(&mvcc, &key), Some((5, 0)));

    // Strictly greater advances.
    mvcc.publish_cell(RecordKey::from(key.clone()), 8).await;
    assert_eq!(cell_state(&mvcc, &key), Some((8, 0)));

    // Equal is a no-op (NOT a regression — but also not an advance).
    mvcc.publish_cell(RecordKey::from(key.clone()), 8).await;
    assert_eq!(cell_state(&mvcc, &key), Some((8, 0)));
}

/// The vacant-insert path is unaffected: a brand-new key still seeds at the
/// offered version.
#[tokio::test(flavor = "multi_thread")]
async fn publish_cell_seeds_vacant_cell() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"cold");
    assert_eq!(cell_state(&mvcc, &key), None);

    mvcc.publish_cell(RecordKey::from(key.clone()), 42).await;
    assert_eq!(cell_state(&mvcc, &key), Some((42, 0)));
}

// ----------------------------------------------------------------
// seed_version
// ----------------------------------------------------------------

/// A2 core interleaving for `seed_version`: the cold-read cache-seed path
/// races a newer overlay-only commit. A history-derived OLDER version
/// offered on top of a fresher in-memory cell MUST NOT regress it. Before
/// the fix, `upsert_async` unconditionally replaced the value.
#[tokio::test(flavor = "multi_thread")]
async fn seed_version_never_regresses_version() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    // 1. Transaction B commits k at version 11 in-memory (cell advanced).
    mvcc.seed_version(RecordKey::from(key.clone()), 11).await;
    assert_eq!(cell_state(&mvcc, &key), Some((11, 0)));

    // 2. A cold read races in and seeds the cell from a STALE history-derived
    //    version 10 (older durable anchor). This MUST NOT regress the cell.
    mvcc.seed_version(RecordKey::from(key.clone()), 10).await;

    assert_eq!(
        cell_state(&mvcc, &key),
        Some((11, 0)),
        "A2: seed_version must be max-monotonic — a stale history-derived \
         seed must not regress the cell's version"
    );
    assert_eq!(mvcc.version_of(&key), 11);
}

/// The max-monotonic guard still lets a strictly-greater version advance
/// the cell via `seed_version`.
#[tokio::test(flavor = "multi_thread")]
async fn seed_version_still_advances_on_greater_version() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"k");

    mvcc.seed_version(RecordKey::from(key.clone()), 5).await;
    assert_eq!(cell_state(&mvcc, &key), Some((5, 0)));

    mvcc.seed_version(RecordKey::from(key.clone()), 9).await;
    assert_eq!(cell_state(&mvcc, &key), Some((9, 0)));

    // Equal is a no-op.
    mvcc.seed_version(RecordKey::from(key.clone()), 9).await;
    assert_eq!(cell_state(&mvcc, &key), Some((9, 0)));
}

/// The vacant-insert path is unaffected: a brand-new key still seeds at the
/// offered version.
#[tokio::test(flavor = "multi_thread")]
async fn seed_version_seeds_vacant_cell() {
    let mvcc = make_mvcc();
    let key = Bytes::from_static(b"cold");
    assert_eq!(cell_state(&mvcc, &key), None);

    mvcc.seed_version(RecordKey::from(key.clone()), 42).await;
    assert_eq!(cell_state(&mvcc, &key), Some((42, 0)));
}
