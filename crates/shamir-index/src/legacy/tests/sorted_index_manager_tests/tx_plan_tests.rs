//! tx-aware forward-equality tests (Stage 3.4) and
//! planner (plan_record_*) tests (Stage 1.1.F).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use crate::legacy::sorted_index_manager::{SortedIndexDefinition, SortedIndexManager};
use crate::write_ops::IndexWriteOp;

use super::helpers::{fresh_mgr, record_with_int};

// ---------------------------------------------------------------------------
// tx-aware forward-equality tests — Stage 3.4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lookup_range_tx_none_equals_lookup_range() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in [3, 1, 7, 5, 2] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    let a = mgr.lookup_range(101, None, None).await.unwrap();
    let b = mgr.lookup_range_tx(0, 101, None, None, None).await.unwrap();
    assert_eq!(a, b);
}

#[tokio::test]
async fn lookup_min_max_tx_none_equal_non_tx() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in [50, 10, 30, 5, 20] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    assert_eq!(
        mgr.lookup_min(101).await.unwrap(),
        mgr.lookup_min_tx(0, 101, None).await.unwrap()
    );
    assert_eq!(
        mgr.lookup_max(101).await.unwrap(),
        mgr.lookup_max_tx(0, 101, None).await.unwrap()
    );
}

#[tokio::test]
async fn lookup_first_last_k_tx_none_equal_non_tx() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in [50, 10, 30, 5, 20, 40] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    assert_eq!(
        mgr.lookup_first_k(101, 3).await.unwrap(),
        mgr.lookup_first_k_tx(0, 101, 3, None).await.unwrap()
    );
    assert_eq!(
        mgr.lookup_last_k(101, 3).await.unwrap(),
        mgr.lookup_last_k_tx(0, 101, 3, None).await.unwrap()
    );
}

// ---------------------------------------------------------------------------
// Planner (plan_record_*) tests — Stage 1.1.F
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_record_created_returns_sorted_posting() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let rid = RecordId::new();
    let rec = record_with_int(201, 42);
    let ops = mgr.plan_record_created(&rid, &rec, 0).unwrap();
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::SetPosting { key, value } => {
            // Key must end with record_id bytes.
            assert_eq!(&key[key.len() - 16..], &rid.to_bytes());
            assert!(value.is_empty());
        }
        other => panic!("expected SetPosting, got {other:?}"),
    }
}

#[tokio::test]
async fn plan_record_deleted_returns_remove_sorted_posting() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let rid = RecordId::new();
    let rec = record_with_int(201, 42);
    let ops = mgr.plan_record_deleted(&rid, &rec).unwrap();
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::RemovePosting { key } => {
            assert_eq!(&key[key.len() - 16..], &rid.to_bytes());
        }
        other => panic!("expected RemovePosting, got {other:?}"),
    }
}

#[tokio::test]
async fn equivalence_plan_apply_vs_direct() {
    // Two managers on separate stores, same definition.
    // One uses on_record_created (wrapper), the other plan + manual apply.
    let store_a: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store_b: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_a = SortedIndexManager::new(Arc::clone(&store_a)).await.unwrap();
    let mgr_b = SortedIndexManager::new(Arc::clone(&store_b)).await.unwrap();
    let def = SortedIndexDefinition::new(101, vec![201]);
    mgr_a.register(def.clone()).await.unwrap();
    mgr_b.register(def).await.unwrap();

    let rid = RecordId::new();
    let rec = record_with_int(201, 77);

    // Direct wrapper path.
    mgr_a.on_record_created(&rid, &rec, 1).await.unwrap();

    // Plan + manual apply path.
    let ops = mgr_b.plan_record_created(&rid, &rec, 1).unwrap();
    for op in &ops {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                store_b
                    .set(key.clone().into(), value.clone())
                    .await
                    .unwrap();
            }
            IndexWriteOp::RemovePosting { key } => {
                let _ = store_b.remove(key.clone().into()).await.unwrap();
            }
            _ => {}
        }
    }

    // Both stores should yield the same lookup results.
    let r_a = mgr_a.lookup_range(101, None, None).await.unwrap();
    let r_b = mgr_b.lookup_range(101, None, None).await.unwrap();
    assert_eq!(r_a, r_b);
    assert!(r_a.contains(&rid));
}
