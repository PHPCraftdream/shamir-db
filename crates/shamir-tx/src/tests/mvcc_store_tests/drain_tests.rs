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

use super::helpers::{count_history_entries, make_mvcc};
use crate::version_codec::encode_version_key;

/// Read the raw value stored in history under `encode_version_key(key, v)`.
async fn history_raw(mvcc: &crate::mvcc_store::MvccStore, key: &[u8], v: u64) -> Option<Bytes> {
    match mvcc.history_store().get(encode_version_key(key, v)).await {
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
        .set_versioned(Bytes::from_static(b"k1"), Bytes::from_static(b"v1"))
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
        .set_versioned(Bytes::from_static(b"a"), Bytes::from_static(b"val-a"))
        .await
        .unwrap();
    let _v2 = mvcc
        .set_versioned(Bytes::from_static(b"b"), Bytes::from_static(b"val-b"))
        .await
        .unwrap();
    let v3 = mvcc
        .set_versioned(Bytes::from_static(b"c"), Bytes::from_static(b"val-c"))
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

    mvcc.set_versioned(Bytes::from_static(b"k"), Bytes::from_static(b"v"))
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

    mvcc.set_versioned(Bytes::from_static(b"k"), Bytes::from_static(b"v1"))
        .await
        .unwrap();

    mvcc.drain_to_history().await.unwrap();

    // Read path must still resolve correctly from history after overlay GC.
    let val = mvcc.get_current(Bytes::from_static(b"k")).await.unwrap();
    assert_eq!(val, Some(Bytes::from_static(b"v1")));
}

/// Drain advances the durable watermark to the visibility watermark.
#[tokio::test]
async fn drain_advances_durable_watermark() {
    let mvcc = make_mvcc();

    mvcc.set_versioned(Bytes::from_static(b"k"), Bytes::from_static(b"v"))
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
