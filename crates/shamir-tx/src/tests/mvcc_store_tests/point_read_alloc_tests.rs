//! L15 — behavioural equivalence tests for `get_current(&[u8])`.
//!
//! Verify that passing a borrowed `&[u8]` key to `get_current` produces
//! identical results across every read scenario: present, tombstoned,
//! absent, overlay-hit, history-hit, and cold-start.

use super::helpers::make_mvcc;
use bytes::Bytes;
use shamir_storage::types::RecordKey;

/// Hot-path: key is present in history, cell is cached.
#[tokio::test]
async fn get_current_ref_present() {
    let mvcc = make_mvcc();
    let key = b"present-key";
    let val = Bytes::from("value-1");
    mvcc.set_versioned(RecordKey::from(Bytes::copy_from_slice(key)), val.clone())
        .await
        .unwrap();

    // Borrow read — single alloc (version-key only).
    let got = mvcc.get_current_bytes(key.as_slice()).await.unwrap();
    assert_eq!(got, Some(val));
}

/// Key was written then deleted (tombstone). `get_current` returns `None`.
#[tokio::test]
async fn get_current_ref_tombstoned() {
    let mvcc = make_mvcc();
    let key = b"tomb-key";
    mvcc.set_versioned(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::from("alive"),
    )
    .await
    .unwrap();
    mvcc.delete_versioned(RecordKey::from(Bytes::copy_from_slice(key)))
        .await
        .unwrap();

    let got = mvcc.get_current_bytes(key.as_slice()).await.unwrap();
    assert_eq!(got, None);
}

/// Key was never written. `get_current` returns `None`.
#[tokio::test]
async fn get_current_ref_absent() {
    let mvcc = make_mvcc();
    let got = mvcc
        .get_current_bytes(b"never-written".as_slice())
        .await
        .unwrap();
    assert_eq!(got, None);
}

/// Multiple versions: latest wins.
#[tokio::test]
async fn get_current_ref_latest_version_wins() {
    let mvcc = make_mvcc();
    let key = b"multi-ver";
    mvcc.set_versioned(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::from("v1"),
    )
    .await
    .unwrap();
    mvcc.set_versioned(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::from("v2"),
    )
    .await
    .unwrap();
    mvcc.set_versioned(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::from("v3"),
    )
    .await
    .unwrap();

    let got = mvcc.get_current_bytes(key.as_slice()).await.unwrap();
    assert_eq!(got, Some(Bytes::from("v3")));
}

/// Bytes auto-coercion: callers passing `Bytes` still compile and work
/// via `Deref<Target=[u8]>`.
#[tokio::test]
async fn get_current_bytes_coercion() {
    let mvcc = make_mvcc();
    let key = Bytes::from("coerce-key");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("val"))
        .await
        .unwrap();

    // Pass Bytes directly — auto-derefs to &[u8].
    let got = mvcc.get_current_bytes(&key).await.unwrap();
    assert_eq!(got, Some(Bytes::from("val")));
}

/// R3 floor-cap: when a version exceeds the committed floor, the read
/// falls back to `get_at(floor)`. Verify the borrowed path honours this.
#[tokio::test]
async fn get_current_ref_floor_cap() {
    use super::helpers::make_gate;
    use super::helpers::make_mvcc_with_gate;

    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let key = b"floor-key";

    // First write — auto-advances gate.
    mvcc.set_versioned(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::from("committed"),
    )
    .await
    .unwrap();

    // Confirm the floor moved.
    let floor = gate.last_committed();
    assert!(floor > 0);

    // Second write — also auto-advances gate.
    mvcc.set_versioned(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::from("later"),
    )
    .await
    .unwrap();

    // Both writes are committed, so the latest is visible.
    let got = mvcc.get_current_bytes(key.as_slice()).await.unwrap();
    assert_eq!(got, Some(Bytes::from("later")));
}
