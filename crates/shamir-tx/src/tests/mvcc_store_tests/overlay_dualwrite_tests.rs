//! P1c — overlay populated on the ack-path, byte-for-byte parity with history.
//!
//! P1b made the three read seams overlay-aware but never populated the overlay
//! on the production path. P1c populates it on EVERY write path (tx
//! `apply_committed_ops`, non-tx `set_versioned` / `set_versioned_many` /
//! `delete_versioned`) while keeping the inline history dual-write.
//!
//! Two proof obligations:
//!
//! 1. **Byte parity.** For every written `(key, version)` the overlay value
//!    and the durable history value at `encode_version_key(key, version)` are
//!    the SAME bytes — including the empty-`Bytes` tombstone for deletes. Any
//!    divergence (missed insert, wrong payload, tombstone mismatch) is a bug.
//!
//! 2. **Overlay actually serves reads.** After a write, deleting the entry
//!    straight out of history must NOT change what `get_at` / `get_current`
//!    return — proving the read resolved from the overlay branch, not history.

use bytes::Bytes;
use shamir_storage::types::KvOp;

use super::helpers::{make_gate, make_mvcc, make_mvcc_with_gate};
use crate::version_codec::encode_version_key;
use shamir_storage::types::RecordKey;

/// Read the raw value stored in history under `encode_version_key(key, v)`.
/// Returns the bytes as written (empty `Bytes` for a tombstone), or `None`
/// when the physical key is absent from the durable log.
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

// ============================================================================
// 1. Byte-parity: overlay value == history value for every written version.
// ============================================================================

/// Non-tx `set_versioned` writes the SAME bytes into overlay and history.
#[tokio::test]
async fn nontx_set_overlay_matches_history() {
    let mvcc = make_mvcc();
    let key = b"k".as_slice();
    let val = Bytes::from_static(b"payload-v");

    let v = mvcc
        .set_versioned(RecordKey::from(Bytes::from_static(b"k")), val.clone())
        .await
        .unwrap();

    let ov = mvcc
        .overlay()
        .get(key, v)
        .expect("overlay must hold the version");
    let hist = history_raw(&mvcc, key, v)
        .await
        .expect("history must hold the version");
    assert_eq!(ov, hist, "overlay vs history byte mismatch");
    assert_eq!(ov, val, "overlay value must be the written payload");
}

/// Non-tx `delete_versioned` writes an EMPTY tombstone into both — and the two
/// tombstones are byte-equal (both empty).
#[tokio::test]
async fn nontx_delete_overlay_tombstone_matches_history() {
    let mvcc = make_mvcc();
    let key = b"k".as_slice();
    // Seed a value first, then delete it.
    mvcc.set_versioned(
        RecordKey::from(Bytes::from_static(b"k")),
        Bytes::from_static(b"v0"),
    )
    .await
    .unwrap();
    let dv = mvcc
        .delete_versioned(RecordKey::from(Bytes::from_static(b"k")))
        .await
        .unwrap();

    let ov = mvcc
        .overlay()
        .get(key, dv)
        .expect("overlay must hold the tombstone version");
    let hist = history_raw(&mvcc, key, dv)
        .await
        .expect("history must hold the tombstone version");
    assert!(ov.is_empty(), "overlay tombstone must be empty");
    assert!(hist.is_empty(), "history tombstone must be empty");
    assert_eq!(ov, hist, "overlay vs history tombstone mismatch");
}

/// Non-tx `set_versioned_many` — every batched pair has overlay == history.
#[tokio::test]
async fn nontx_set_many_overlay_matches_history() {
    let mvcc = make_mvcc();
    let items = vec![
        (Bytes::from_static(b"a"), Bytes::from_static(b"va")),
        (Bytes::from_static(b"b"), Bytes::from_static(b"vb")),
        (Bytes::from_static(b"c"), Bytes::from_static(b"vc")),
    ];
    let max_v = mvcc
        .set_versioned_many(
            items
                .clone()
                .into_iter()
                .map(|(k, v)| (RecordKey::from(k), v))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    // Versions are assigned sequentially; the batch occupies
    // [max_v - n + 1 ..= max_v] in order of `items`.
    let n = items.len() as u64;
    let base = max_v - n + 1;
    for (i, (key, val)) in items.iter().enumerate() {
        let v = base + i as u64;
        let ov = mvcc
            .overlay()
            .get(key, v)
            .unwrap_or_else(|| panic!("overlay missing version {v} for key {key:?}"));
        let hist = history_raw(&mvcc, key, v)
            .await
            .unwrap_or_else(|| panic!("history missing version {v} for key {key:?}"));
        assert_eq!(ov, hist, "overlay vs history mismatch for {key:?}@{v}");
        assert_eq!(&ov, val);
    }
}

/// tx-path `apply_committed_ops` — Set and Remove ops both land byte-identical
/// in overlay and history.
#[tokio::test]
async fn tx_apply_committed_overlay_matches_history() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let commit_version = gate.assign_next_version();

    let ops = vec![
        KvOp::Set(
            Bytes::from_static(b"s1").into(),
            Bytes::from_static(b"set-1"),
        ),
        KvOp::Set(
            Bytes::from_static(b"s2").into(),
            Bytes::from_static(b"set-2"),
        ),
        KvOp::Remove(Bytes::from_static(b"d1").into()),
    ];
    mvcc.apply_committed_ops(ops.clone(), commit_version)
        .await
        .unwrap();

    for op in &ops {
        let (key, expected): (&[u8], Bytes) = match op {
            KvOp::Set(k, v) => (k, v.clone()),
            KvOp::Remove(k) => (k, Bytes::new()),
        };
        let ov = mvcc
            .overlay()
            .get(key, commit_version)
            .unwrap_or_else(|| panic!("overlay missing {key:?}@{commit_version}"));
        let hist = history_raw(&mvcc, key, commit_version)
            .await
            .unwrap_or_else(|| panic!("history missing {key:?}@{commit_version}"));
        assert_eq!(ov, hist, "overlay vs history mismatch for {key:?}");
        assert_eq!(ov, expected, "overlay value mismatch for {key:?}");
    }
}

// ============================================================================
// 2. Overlay actually serves reads — delete from history, value still visible.
// ============================================================================

/// After a non-tx `set_versioned`, removing the durable history entry leaves
/// `get_at` (and `get_current`) still returning the value — proving the read
/// resolved from the overlay branch, not from history.
#[tokio::test]
async fn overlay_serves_read_after_history_removed() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let val = Bytes::from_static(b"only-in-overlay");
    let v = mvcc
        .set_versioned(RecordKey::from(Bytes::from_static(b"k")), val.clone())
        .await
        .unwrap();

    // Yank the durable entry straight out of history. If the read path still
    // consulted history it would now see nothing; the overlay must cover it.
    mvcc.history_store()
        .remove(encode_version_key(b"k", v).into())
        .await
        .unwrap();
    // Sanity: history really is empty for this version now.
    assert!(
        history_raw(&mvcc, b"k", v).await.is_none(),
        "precondition: history entry must be gone",
    );

    // Direct-path snapshot read at the write version: must come from overlay.
    assert_eq!(
        mvcc.get_at(b"k", v).await.unwrap(),
        Some(val.clone()),
        "get_at must resolve the value from the overlay after history removal",
    );
    // Current read (cell points at `v`): also overlay-served.
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(val),
        "get_current must resolve the value from the overlay after history removal",
    );
}

/// Same proof for a tx-committed value: remove the history entry and the
/// overlay still serves the snapshot read.
#[tokio::test]
async fn overlay_serves_tx_read_after_history_removed() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let commit_version = gate.assign_next_version();
    let val = Bytes::from_static(b"tx-overlay-only");
    mvcc.apply_committed_ops(
        vec![KvOp::Set(Bytes::from_static(b"tk").into(), val.clone())],
        commit_version,
    )
    .await
    .unwrap();

    mvcc.history_store()
        .remove(encode_version_key(b"tk", commit_version).into())
        .await
        .unwrap();
    assert!(
        history_raw(&mvcc, b"tk", commit_version).await.is_none(),
        "precondition: history entry must be gone",
    );

    assert_eq!(
        mvcc.get_at(b"tk", commit_version).await.unwrap(),
        Some(val),
        "tx value must be overlay-served after history removal",
    );
}

/// Tombstone proof: after a non-tx delete, removing the history tombstone
/// leaves the overlay tombstone in charge → the key still reads as deleted.
#[tokio::test]
async fn overlay_serves_tombstone_after_history_removed() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    mvcc.set_versioned(
        RecordKey::from(Bytes::from_static(b"k")),
        Bytes::from_static(b"v0"),
    )
    .await
    .unwrap();
    let dv = mvcc
        .delete_versioned(RecordKey::from(Bytes::from_static(b"k")))
        .await
        .unwrap();

    // Remove the durable tombstone. The overlay tombstone must still suppress.
    mvcc.history_store()
        .remove(encode_version_key(b"k", dv).into())
        .await
        .unwrap();
    assert!(
        history_raw(&mvcc, b"k", dv).await.is_none(),
        "precondition: history tombstone must be gone",
    );

    assert_eq!(
        mvcc.get_at(b"k", dv).await.unwrap(),
        None,
        "overlay tombstone must keep the key deleted after history removal",
    );
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        None,
        "get_current must honour the overlay tombstone after history removal",
    );
}
