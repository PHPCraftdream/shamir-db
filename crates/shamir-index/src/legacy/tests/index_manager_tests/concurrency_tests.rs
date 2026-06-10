use super::helpers::{create_manager, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_manager::IndexManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

// ============================================================================
// Concurrency test
// ============================================================================

#[tokio::test]
async fn test_concurrent_index_operations() {
    let (_, _, manager) = create_manager();

    // Create index first
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Spawn multiple concurrent tasks
    let mut handles = Vec::new();
    for i in 0..10 {
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            let value = create_test_value(&[(1, InnerValue::Int(i))]);
            let record_id = RecordId::new();

            // Create
            mgr.on_record_created(&record_id, &value).await.unwrap();

            // Lookup
            let result = mgr
                .lookup_by_index(1001, &[InnerValue::Int(i)])
                .await
                .unwrap();
            assert_eq!(result.len(), 1);

            // Delete
            mgr.on_record_deleted(&record_id, &value).await.unwrap();

            // Verify deleted
            let result = mgr
                .lookup_by_index(1001, &[InnerValue::Int(i)])
                .await
                .unwrap();
            assert!(result.is_empty());
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_concurrent_unique_index_validation() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // One record exists
    let existing = create_test_value(&[(1, InnerValue::Str("exists".to_string()))]);
    let existing_id = RecordId::new();
    manager
        .on_record_created_unique(&existing_id, &existing)
        .await
        .unwrap();

    // Multiple concurrent validations
    let mut handles = Vec::new();
    for _ in 0..5 {
        let mgr = manager.clone();
        let val = existing.clone();
        let handle = tokio::spawn(async move {
            // All should fail because value exists
            let result = mgr.validate_unique_for_create(&val).await;
            result
        });
        handles.push(handle);
    }

    // All should fail
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_err());
    }
}

/// Opt G — posting-list cache correctness.
///
/// Reads should return the freshest BTreeSet after every write. The
/// hash-keyed in-memory cache is invalidated on `on_record_*` hooks;
/// these tests pin the invalidation paths so a future "forgot to drop
/// the cache" regression fails immediately.
#[tokio::test]
async fn test_cache_invalidated_on_create() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(def).await.unwrap();

    // Prime the cache with an empty result.
    let r0 = manager
        .lookup_by_index(1001, &[InnerValue::Int(42)])
        .await
        .unwrap();
    assert!(r0.is_empty());

    // Write — must invalidate the cached empty result.
    let id = RecordId::new();
    let val = create_test_value(&[(1, InnerValue::Int(42))]);
    manager.on_record_created(&id, &val).await.unwrap();

    let r1 = manager
        .lookup_by_index(1001, &[InnerValue::Int(42)])
        .await
        .unwrap();
    assert_eq!(r1.len(), 1, "create must invalidate the cached empty set");
    assert!(r1.contains(&id));
}

#[tokio::test]
async fn test_cache_invalidated_on_delete() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(def).await.unwrap();

    let id = RecordId::new();
    let val = create_test_value(&[(1, InnerValue::Int(7))]);
    manager.on_record_created(&id, &val).await.unwrap();

    // Prime cache with the populated result.
    let r0 = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap();
    assert_eq!(r0.len(), 1);

    // Delete — must invalidate.
    manager.on_record_deleted(&id, &val).await.unwrap();
    let r1 = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap();
    assert!(
        r1.is_empty(),
        "delete must invalidate the stale (id-still-present) cached set"
    );
}

#[tokio::test]
async fn test_cache_invalidated_on_update_value_change() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(def).await.unwrap();

    let id = RecordId::new();
    let old_val = create_test_value(&[(1, InnerValue::Int(10))]);
    let new_val = create_test_value(&[(1, InnerValue::Int(20))]);
    manager.on_record_created(&id, &old_val).await.unwrap();

    // Prime the cached old-bucket and the (empty) new-bucket.
    assert_eq!(
        manager
            .lookup_by_index(1001, &[InnerValue::Int(10)])
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(manager
        .lookup_by_index(1001, &[InnerValue::Int(20)])
        .await
        .unwrap()
        .is_empty());

    // Update — value moves from 10 to 20. Both cache entries must
    // be invalidated for subsequent reads to be correct.
    manager
        .on_record_updated(&id, &old_val, &new_val)
        .await
        .unwrap();

    assert!(
        manager
            .lookup_by_index(1001, &[InnerValue::Int(10)])
            .await
            .unwrap()
            .is_empty(),
        "old-bucket cache must be invalidated"
    );
    assert_eq!(
        manager
            .lookup_by_index(1001, &[InnerValue::Int(20)])
            .await
            .unwrap()
            .len(),
        1,
        "new-bucket cache must be invalidated"
    );
}

#[tokio::test]
async fn test_concurrent_reads_with_index() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Add some records
    for i in 0..20i64 {
        let value = create_test_value(&[(1, InnerValue::Int(i))]);
        let record_id = RecordId::new();
        manager.on_record_created(&record_id, &value).await.unwrap();
    }

    // Concurrent reads
    let mut handles = Vec::new();
    for _ in 0..10 {
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            for j in 0..20i64 {
                let result = mgr
                    .lookup_by_index(1001, &[InnerValue::Int(j)])
                    .await
                    .unwrap();
                assert_eq!(result.len(), 1);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

// ============================================================================
// plan_* tests (Stage 1.1.E)
// ============================================================================

#[tokio::test]
async fn plan_record_created_returns_postings() {
    use crate::write_ops::IndexWriteOp;

    let (_, _, manager) = create_manager();
    let idx1 = IndexDefinition::new(2001, vec![IndexInfoItem::new(vec![1])]);
    let idx2 = IndexDefinition::new(2002, vec![IndexInfoItem::new(vec![2])]);
    manager.create_index(idx1).await.unwrap();
    manager.create_index(idx2).await.unwrap();

    let record_id = RecordId::new();
    let value = create_test_value(&[
        (1, InnerValue::Str("hello".to_string())),
        (2, InnerValue::Int(42)),
    ]);

    let ops = manager
        .plan_record_created(&record_id, &value)
        .await
        .unwrap();

    // One SetPosting per index definition
    assert_eq!(ops.len(), 2);
    for op in &ops {
        match op {
            IndexWriteOp::SetPosting { value, .. } => {
                assert!(value.is_empty(), "posting value should be empty");
            }
            other => panic!("expected SetPosting, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn plan_record_deleted_returns_removes() {
    use crate::write_ops::IndexWriteOp;

    let (_, _, manager) = create_manager();
    let idx = IndexDefinition::new(3001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(idx).await.unwrap();

    let record_id = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("bye".to_string()))]);

    let ops = manager
        .plan_record_deleted(&record_id, &value)
        .await
        .unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::RemovePosting { .. } => {}
        other => panic!("expected RemovePosting, got {:?}", other),
    }
}

#[tokio::test]
async fn equivalence_plan_apply_vs_direct() {
    let data1 = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info1 = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let mgr1 = IndexManager::new(Arc::clone(&data1), Arc::clone(&info1))
        .await
        .unwrap();

    let data2 = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info2 = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let mgr2 = IndexManager::new(Arc::clone(&data2), Arc::clone(&info2))
        .await
        .unwrap();

    let idx = IndexDefinition::new(4001, vec![IndexInfoItem::new(vec![1])]);
    mgr1.create_index(idx.clone()).await.unwrap();
    mgr2.create_index(idx).await.unwrap();

    let rid = RecordId::new();
    let val = create_test_value(&[(1, InnerValue::Str("equiv".to_string()))]);

    // mgr1: on_record_created (wrapper — internally plan+apply)
    mgr1.on_record_created(&rid, &val).await.unwrap();

    // mgr2: explicit plan + manual apply via store
    let ops = mgr2.plan_record_created(&rid, &val).await.unwrap();
    assert!(!ops.is_empty());
    for op in &ops {
        match op {
            crate::write_ops::IndexWriteOp::SetPosting { key, value } => {
                info2.set(key.clone(), value.clone()).await.unwrap();
            }
            crate::write_ops::IndexWriteOp::RemovePosting { key } => {
                let _ = info2.remove(key.clone()).await;
            }
            _ => {}
        }
    }

    // Both should return the same lookup result.
    let r1 = mgr1
        .lookup_by_index(4001, &[InnerValue::Str("equiv".to_string())])
        .await
        .unwrap();
    let r2 = mgr2
        .lookup_by_index(4001, &[InnerValue::Str("equiv".to_string())])
        .await
        .unwrap();
    assert_eq!(r1, r2);
    assert!(r1.contains(&rid));
}
