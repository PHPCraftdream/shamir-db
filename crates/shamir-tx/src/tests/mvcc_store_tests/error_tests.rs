use super::helpers::make_gate;
use super::test_stores::make_failing_history_mvcc;
use bytes::Bytes;
use std::sync::atomic::Ordering;
use shamir_storage::types::RecordKey;

// ================================================================
// Regression tests — I/O error propagation (fault injection).
// ================================================================

/// Regression: `delete_versioned` propagates `history.set()` errors.
///
/// The log is the sole durable write. If `history.set()` (the tombstone
/// write) fails, the error propagates and the delete is treated as if it
/// never happened — no stale cell, caller sees `Err`.
#[tokio::test]
async fn delete_versioned_propagates_remove_error() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    // Arm: the next `set` call on history will fail (tombstone write).
    history.fail_set.store(true, Ordering::Relaxed);

    let result = mvcc.delete_versioned(RecordKey::from(Bytes::from("k"))).await;
    assert!(
        result.is_err(),
        "delete_versioned must propagate history.set() I/O error"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("injected"),
        "error should be the injected fault, got: {err_msg}"
    );
}

/// Regression: `set_versioned` propagates `history.set()` errors.
///
/// The log is the sole durable write. If `history.set()` fails, the error
/// propagates and no version is committed.
#[tokio::test]
async fn set_versioned_propagates_archive_read_error() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    // Arm: the next `set` call on history will fail (log write).
    history.fail_set.store(true, Ordering::Relaxed);

    let result = mvcc
        .set_versioned(RecordKey::from(Bytes::from("k")), Bytes::from("new_val"))
        .await;
    assert!(
        result.is_err(),
        "set_versioned must propagate history.set() I/O error"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("injected"),
        "error should be the injected fault, got: {err_msg}"
    );

    // Nothing was written — get_current returns None.
    history.fail_set.store(false, Ordering::Relaxed);
    let via_seam = mvcc.get_current(RecordKey::from(Bytes::from("k"))).await.unwrap();
    assert!(
        via_seam.is_none(),
        "FINAL-A: key must be absent when history.set() fails"
    );
}

/// Regression: `delete_versioned` propagates `history.set()` (tombstone).
///
/// Mirrors `set_versioned_propagates_archive_read_error` for the delete path.
/// The caller sees `Err` when the log write fails — the error is not swallowed.
#[tokio::test]
async fn delete_versioned_propagates_archive_read_error() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    // Arm: next history write fails → tombstone can't land.
    history.fail_set.store(true, Ordering::Relaxed);

    let result = mvcc.delete_versioned(RecordKey::from(Bytes::from("k"))).await;
    assert!(
        result.is_err(),
        "delete_versioned must propagate history.set() I/O error (tombstone)"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("injected"),
        "error should be the injected fault, got: {err_msg}"
    );
}
