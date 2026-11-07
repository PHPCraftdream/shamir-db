use std::future::{poll_fn, Future};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::RecordKey;

use crate::mvcc_store::write_receipt::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use crate::{MvccStore, RepoTxGate};

fn fixture() -> (Arc<RepoTxGate>, MvccStore) {
    let gate = Arc::new(RepoTxGate::new(0, 1));
    let mvcc = MvccStore::new(Arc::new(InMemoryStore::new()), Arc::clone(&gate));
    mvcc.set_test_now(1_000);
    (gate, mvcc)
}

#[tokio::test(start_paused = true)]
async fn receipt_expiry_restores_registration_and_settlement_wins_over_reopen() {
    let (gate, mvcc) = fixture();
    let lower = gate.assign_next_version_guarded();
    let operation = mvcc.set_versioned_with_receipt(
        RecordKey::from_slice(b"key"),
        Bytes::from_static(b"committed"),
        VisibilityWait::Bounded { timeout_ms: 100 },
    );
    tokio::pin!(operation);
    assert!(poll_fn(|cx| Poll::Ready(operation.as_mut().poll(cx)))
        .await
        .is_pending());
    assert_eq!(gate.completion().waiter_count(), 1);
    tokio::time::advance(Duration::from_millis(100)).await;
    let receipt = operation.await.unwrap();
    assert_eq!(receipt.commit_version, Some(2));
    assert_eq!(
        receipt.visibility,
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        },
    );
    assert_eq!(gate.completion().waiter_count(), 0);
    assert_eq!(
        mvcc.get_at(b"key", 2).await.unwrap(),
        Some(Bytes::from_static(b"committed")),
    );
    gate.set_requires_reopen();
    lower.commit();
    assert_eq!(
        mvcc.await_non_tx_ack(2, VisibilityWait::Strict).await,
        VisibilityState::Visible,
    );
}

// Counterfactual: sampling timeout/reopen before the final W sample reports
// Pending even though every version through target has settled at expiry.
#[tokio::test(start_paused = true)]
async fn expiry_final_sample_prioritizes_settlement() {
    let (gate, mvcc) = fixture();
    let lower = gate.assign_next_version_guarded();
    let operation = mvcc.set_versioned_with_receipt(
        RecordKey::from_slice(b"key"),
        Bytes::from_static(b"written"),
        VisibilityWait::Bounded { timeout_ms: 100 },
    );
    tokio::pin!(operation);
    assert!(poll_fn(|cx| Poll::Ready(operation.as_mut().poll(cx)))
        .await
        .is_pending());
    tokio::time::advance(Duration::from_millis(100)).await;
    gate.set_requires_reopen();
    lower.commit();
    assert_eq!(
        operation.await.unwrap().visibility,
        VisibilityState::Visible
    );
    assert_eq!(gate.completion().waiter_count(), 0);
}

#[tokio::test]
async fn immediate_no_wait_receipts_preserve_many_delete_and_empty_positions() {
    let (gate, mvcc) = fixture();
    let lower = gate.assign_next_version_guarded();
    let many = mvcc
        .set_versioned_many_no_wait_with_receipt(vec![
            (RecordKey::from_slice(b"a"), Bytes::from_static(b"a")),
            (RecordKey::from_slice(b"b"), Bytes::from_static(b"b")),
        ])
        .await
        .unwrap();
    assert_eq!(many.commit_version, Some(3));
    assert_eq!(
        many.visibility,
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        },
    );
    let deleted = mvcc
        .delete_versioned_no_wait_with_receipt(RecordKey::from_slice(b"a"))
        .await
        .unwrap();
    assert_eq!(deleted.commit_version, Some(4));
    assert_eq!(deleted.visibility, many.visibility);
    assert_eq!(mvcc.get_at(b"a", 4).await.unwrap(), None);
    let empty = mvcc
        .set_versioned_many_with_receipt(Vec::new(), VisibilityWait::Strict)
        .await
        .unwrap();
    assert_eq!(empty.commit_version, None);
    assert_eq!(empty.visibility, VisibilityState::Visible);
    assert_eq!(gate.version_allocation_high_water_mark(), 4);
    assert_eq!(gate.completion().waiter_registrations(), 0);
    lower.commit();
    assert_eq!(gate.visible_floor(), 4);
}
