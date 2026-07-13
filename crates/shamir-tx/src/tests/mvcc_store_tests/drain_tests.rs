//! Phase F.2 — `drain_to_history` unit tests.
//!
//! `drain_to_history` is the synchronous overlay→history drain used by
//! `rename_table_stores` to ensure every committed overlay entry is durable
//! in `__history__` before the store is copied to the new table name.
//!
//! Proof obligations:
//! 1. After drain, the overlay is empty (all entries reclaimed by `gc_overlay_to`).
//! 2. After drain, history holds the value for every drained version.
//! 3. Drain is idempotent — a second call on an already-drained store is a no-op.
//! 4. The durable watermark advances to the visibility watermark after drain.

use bytes::Bytes;
use shamir_storage::types::KvOp;

use super::helpers::{count_history_entries, make_gate, make_mvcc, make_mvcc_with_gate};
use crate::version_codec::encode_version_key;
use shamir_storage::types::RecordKey;

/// Read the raw value stored in history under `encode_version_key(key, v)`.
async fn history_raw(mvcc: &crate::mvcc_store::MvccStore, key: &[u8], v: u64) -> Option<Bytes> {
    match mvcc
        .history_store()
        .get(encode_version_key(key, v).into())
        .await
    {
        Ok(val) => Some(val),
        Err(shamir_storage::error::DbError::NotFound(_)) => None,
        Err(e) => panic!("unexpected history error: {e:?}"),
    }
}

/// After draining a store with one written key, the overlay is empty and
/// history holds the value.
#[tokio::test]
async fn drain_empties_overlay_and_lands_in_history() {
    let mvcc = make_mvcc();

    // Write a key — populates overlay AND history (non-tx inline dual-write).
    let v = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"k1")),
            Bytes::from_static(b"v1"),
        )
        .await
        .unwrap();

    // Overlay should be non-empty before drain.
    assert_eq!(
        mvcc.overlay_len(),
        1,
        "overlay should hold 1 entry pre-drain"
    );

    // Drain.
    mvcc.drain_to_history().await.unwrap();

    // Overlay is now empty.
    assert_eq!(mvcc.overlay_len(), 0, "overlay must be empty post-drain");

    // History still holds the value (drain did not delete it).
    let hist_val = history_raw(&mvcc, b"k1", v).await;
    assert_eq!(
        hist_val,
        Some(Bytes::from_static(b"v1")),
        "history must hold the drained value"
    );
}

/// Drain lands ALL keys, not just one — verify with multiple writes.
#[tokio::test]
async fn drain_lands_multiple_keys() {
    let mvcc = make_mvcc();

    let _v1 = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"a")),
            Bytes::from_static(b"val-a"),
        )
        .await
        .unwrap();
    let _v2 = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"b")),
            Bytes::from_static(b"val-b"),
        )
        .await
        .unwrap();
    let v3 = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"c")),
            Bytes::from_static(b"val-c"),
        )
        .await
        .unwrap();

    assert_eq!(mvcc.overlay_len(), 3, "overlay should hold 3 entries");

    mvcc.drain_to_history().await.unwrap();

    assert_eq!(mvcc.overlay_len(), 0, "overlay must be empty post-drain");

    // All three values must be readable from history.
    assert_eq!(
        history_raw(&mvcc, b"a", 1).await,
        Some(Bytes::from_static(b"val-a"))
    );
    assert_eq!(
        history_raw(&mvcc, b"b", 2).await,
        Some(Bytes::from_static(b"val-b"))
    );
    assert_eq!(
        history_raw(&mvcc, b"c", v3).await,
        Some(Bytes::from_static(b"val-c"))
    );
}

/// A second `drain_to_history` call on an already-drained store is a no-op:
/// no panic, no error, overlay stays empty, history unchanged.
#[tokio::test]
async fn drain_is_idempotent() {
    let mvcc = make_mvcc();

    mvcc.set_versioned(
        RecordKey::from(Bytes::from_static(b"k")),
        Bytes::from_static(b"v"),
    )
    .await
    .unwrap();

    // First drain.
    mvcc.drain_to_history().await.unwrap();
    assert_eq!(mvcc.overlay_len(), 0);

    let hist_count_after_first = count_history_entries(&mvcc).await;

    // Second drain — must be a no-op.
    mvcc.drain_to_history().await.unwrap();
    assert_eq!(
        mvcc.overlay_len(),
        0,
        "overlay must stay empty after re-drain"
    );

    let hist_count_after_second = count_history_entries(&mvcc).await;
    assert_eq!(
        hist_count_after_first, hist_count_after_second,
        "history entry count must not change on idempotent re-drain"
    );
}

/// Drain on a fresh (never-written) store is a no-op — no panic, no error.
#[tokio::test]
async fn drain_on_empty_store_is_noop() {
    let mvcc = make_mvcc();

    // No writes — overlay is empty, visibility watermark is 0.
    mvcc.drain_to_history().await.unwrap();

    assert_eq!(mvcc.overlay_len(), 0);
    assert_eq!(count_history_entries(&mvcc).await, 0);
}

/// After drain, `get_current` still returns the correct value — the
/// read path resolves from history (overlay miss → history fallback).
#[tokio::test]
async fn get_current_works_after_drain() {
    let mvcc = make_mvcc();

    mvcc.set_versioned(
        RecordKey::from(Bytes::from_static(b"k")),
        Bytes::from_static(b"v1"),
    )
    .await
    .unwrap();

    mvcc.drain_to_history().await.unwrap();

    // Read path must still resolve correctly from history after overlay GC.
    let val = mvcc
        .get_current(RecordKey::from(Bytes::from_static(b"k")))
        .await
        .unwrap();
    assert_eq!(val, Some(Bytes::from_static(b"v1")));
}

/// Drain advances the durable watermark to the visibility watermark.
#[tokio::test]
async fn drain_advances_durable_watermark() {
    let mvcc = make_mvcc();

    mvcc.set_versioned(
        RecordKey::from(Bytes::from_static(b"k")),
        Bytes::from_static(b"v"),
    )
    .await
    .unwrap();

    let visibility = mvcc.gate.last_committed();
    assert!(
        visibility > 0,
        "visibility watermark must be > 0 after write"
    );

    // Drain should advance durable watermark to visibility.
    mvcc.drain_to_history().await.unwrap();

    let durable = mvcc.gate.durable_watermark();
    assert_eq!(
        durable, visibility,
        "durable watermark must equal visibility after drain"
    );
}

/// CRIT-3 (#437): `drain_to_history` on ONE table must NOT advance the
/// (repo-shared) durable watermark past a DIFFERENT table's still-undrained
/// version. `self.gate` is a single `Arc<RepoTxGate>` shared across every
/// table in a repo (mirrors production: `per_table_mvcc` stores share one
/// gate) — pre-fix, `drain_to_history` called `gate.mark_durable(visibility)`
/// where `visibility` is the repo-GLOBAL last_committed, not this table's own
/// drained versions. If table B commits AFTER table A's last drained
/// version but BEFORE A's `drain_to_history` runs (the RENAME-race the
/// audit describes), the pre-fix code would mark B's version durable
/// despite never having drained B's overlay — opening the door for a
/// repo-wide overlay GC to erase B's only RAM copy before it ever reaches
/// history.
#[tokio::test]
async fn drain_on_one_table_does_not_advance_watermark_past_another_undrained_table() {
    let gate = make_gate();
    let table_a = make_mvcc_with_gate(gate.clone());
    let table_b = make_mvcc_with_gate(gate.clone());

    // Both tables commit via `apply_committed_visible` + a manual
    // `VersionGuard` — this is the REAL tx-commit ack-path shape (populate
    // the overlay, mark visibility Materialized) that leaves durability
    // to a LATER drain (the background Drainer, or here, `drain_to_history`
    // on RENAME). `set_versioned` (used by the OTHER tests in this file) is
    // the "non-tx" path and marks `mark_durable` INLINE at write time
    // (`mvcc_store/mod.rs::set_versioned`) — it would make every version
    // durable immediately, defeating the whole point of this test (there
    // would be nothing left "undrained" for table A's drain to wrongly
    // mark durable).
    //
    // Table A commits first (version 1) — visible, NOT durable.
    let guard_a = table_a.gate.assign_next_version_guarded();
    let v_a = guard_a.version();
    table_a.apply_committed_visible(
        &[KvOp::Set(
            Bytes::from_static(b"a-key").into(),
            Bytes::from_static(b"a-val"),
        )],
        v_a,
    );
    guard_a.commit();

    // Table B commits second (version 2) — visible, NOT durable. This is
    // the version drain_to_history on A must NOT mark durable, since A
    // never touched B's overlay.
    let guard_b = table_b.gate.assign_next_version_guarded();
    let v_b = guard_b.version();
    table_b.apply_committed_visible(
        &[KvOp::Set(
            Bytes::from_static(b"b-key").into(),
            Bytes::from_static(b"b-val"),
        )],
        v_b,
    );
    guard_b.commit();
    assert!(v_b > v_a, "table B's commit must be the later version");
    assert_eq!(
        gate.durable_watermark(),
        0,
        "test setup: neither commit is durable yet — both are only visible"
    );

    // Repo-global visibility now covers BOTH tables' versions.
    assert_eq!(gate.last_committed(), v_b);

    // Drain ONLY table A (the RENAME path drains the table being renamed,
    // not every table in the repo).
    table_a.drain_to_history().await.unwrap();

    // THE FIX: the shared durable watermark must NOT have jumped to v_b —
    // table B's overlay entry was never drained by this call. Pre-fix, this
    // would have incorrectly advanced to v_b (the repo-global `visibility`
    // at the time of A's drain).
    let durable = gate.durable_watermark();
    assert!(
        durable < v_b,
        "CRIT-3 regression: draining table A alone must not mark table B's \
         undrained version {v_b} durable (got durable_watermark={durable}) — \
         this would let a repo-wide overlay GC erase B's only RAM copy \
         before it ever reaches history"
    );
    assert_eq!(
        durable, v_a,
        "the durable watermark must advance to exactly table A's own \
         drained version, no further"
    );

    // Table B's overlay entry must still be present and readable — the
    // failure mode this guards against is exactly its premature erasure.
    assert_eq!(
        table_b.overlay_len(),
        1,
        "table B's overlay entry must survive table A's drain untouched"
    );
    let b_val = table_b
        .get_current(RecordKey::from(Bytes::from_static(b"b-key")))
        .await
        .unwrap();
    assert_eq!(
        b_val,
        Some(Bytes::from_static(b"b-val")),
        "table B's value must still be readable after table A's drain"
    );

    // Now drain table B too — the shared watermark must catch up to v_b.
    table_b.drain_to_history().await.unwrap();
    assert_eq!(
        gate.durable_watermark(),
        v_b,
        "once B is ALSO drained, the shared watermark must advance to v_b"
    );
}
