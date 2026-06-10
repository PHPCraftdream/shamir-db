use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

use crate::migration::shadow_log::{MigrationShadowLog, ShadowOp};

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn append_and_read_back() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m1".into(), store);

    let id1 = RecordId::new();
    let id2 = RecordId::new();
    let lsn1 = log
        .append(ShadowOp::Put {
            record_id: id1,
            value: b"hello".to_vec(),
        })
        .await
        .unwrap();
    let lsn2 = log
        .append(ShadowOp::Delete { record_id: id2 })
        .await
        .unwrap();

    assert_eq!(lsn1, 1);
    assert_eq!(lsn2, 2);
    assert_eq!(log.current_lsn(), 2);

    let entries = log.read_from(1).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].lsn, 1);
    assert_eq!(entries[1].lsn, 2);
}

#[tokio::test]
async fn read_from_filters_by_lsn() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m2".into(), store);

    for _ in 0..5 {
        log.append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    }

    let entries = log.read_from(3).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].lsn, 3);
    assert_eq!(entries[1].lsn, 4);
    assert_eq!(entries[2].lsn, 5);
}

#[tokio::test]
async fn append_batch_allocates_sequential_lsns() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m3".into(), store);

    let ops = vec![
        ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"a".to_vec(),
        },
        ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"b".to_vec(),
        },
        ShadowOp::Delete {
            record_id: RecordId::new(),
        },
    ];
    let lsns = log.append_batch(ops).await.unwrap();
    assert_eq!(lsns, vec![1, 2, 3]);
    assert_eq!(log.current_lsn(), 3);

    let entries = log.read_from(1).await.unwrap();
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn purge_removes_all_entries() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m4".into(), store);

    for _ in 0..3 {
        log.append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    }

    let removed = log.purge().await.unwrap();
    assert_eq!(removed, 3);

    let entries = log.read_from(1).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn recover_restores_lsn_counter() {
    let store = mem_store();
    {
        let log = MigrationShadowLog::new("m5".into(), Arc::clone(&store));
        for _ in 0..5 {
            log.append(ShadowOp::Delete {
                record_id: RecordId::new(),
            })
            .await
            .unwrap();
        }
    }
    let log2 = MigrationShadowLog::recover("m5".into(), store)
        .await
        .unwrap();
    assert_eq!(log2.next_lsn(), 6);

    let lsn = log2
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    assert_eq!(lsn, 6);
}

#[tokio::test]
async fn separate_migration_ids_are_isolated() {
    let store = mem_store();
    let log_a = MigrationShadowLog::new("a".into(), Arc::clone(&store));
    let log_b = MigrationShadowLog::new("b".into(), store);

    log_a
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    log_b
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    log_b
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();

    assert_eq!(log_a.read_from(1).await.unwrap().len(), 1);
    assert_eq!(log_b.read_from(1).await.unwrap().len(), 2);
}
