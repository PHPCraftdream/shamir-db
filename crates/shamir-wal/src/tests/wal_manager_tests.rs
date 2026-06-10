use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use crate::active_key::WalActiveKey;
use crate::wal_entry::WalOp;
use crate::wal_entry_any::WalEntryAny;
use crate::wal_entry_v2::{WalEntryV2, WalOpV2};
use crate::wal_manager::WalManager;

fn fresh() -> WalManager {
    WalManager::new(Arc::new(InMemoryStore::new()))
}

#[tokio::test]
async fn begin_commit_leaves_no_inflight() {
    let wal = fresh();
    let txn_id = wal.fresh_txn_id();
    let ops = WalManager::ops_record_created(&[RecordId::new(), RecordId::new()]);
    wal.begin(txn_id, ops).await.unwrap();
    let inflight = wal.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);
    wal.commit(txn_id).await.unwrap();
    let inflight = wal.list_inflight().await.unwrap();
    assert!(inflight.is_empty(), "commit must remove the marker");
}

#[tokio::test]
async fn begin_without_commit_visible_after_reopen() {
    // Same `info_store` Arc — emulates re-opening with the same
    // backend instance; the marker survives because info_store
    // does. In a real on-disk backend the marker survives a
    // process restart for the same reason.
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let wal1 = WalManager::new(info.clone());
    let txn_id = wal1.fresh_txn_id();
    wal1.begin(
        txn_id,
        vec![WalOp::RecordCreated {
            record_id: RecordId::new(),
        }],
    )
    .await
    .unwrap();
    // No commit.
    let wal2 = WalManager::new(info);
    let inflight = wal2.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(inflight[0].txn_id(), txn_id);
    let WalEntryAny::V1(ref v1) = inflight[0] else {
        panic!("expected V1 entry");
    };
    assert!(matches!(v1.ops[0], WalOp::RecordCreated { .. }));
}

#[tokio::test]
async fn commit_is_idempotent() {
    let wal = fresh();
    let txn_id = wal.fresh_txn_id();
    wal.begin(txn_id, vec![]).await.unwrap();
    wal.commit(txn_id).await.unwrap();
    // Second commit on an already-removed marker — must not error.
    wal.commit(txn_id).await.unwrap();
}

#[tokio::test]
async fn commit_async_eventually_removes_marker() {
    let wal = fresh();
    let txn_id = wal.fresh_txn_id();
    wal.begin(txn_id, vec![]).await.unwrap();
    assert_eq!(wal.list_inflight().await.unwrap().len(), 1);

    let handle = wal.commit_async(txn_id);
    // Marker may or may not be gone yet — we don't block here
    // (the point of commit_async). After awaiting the handle
    // it MUST be gone.
    handle.await.unwrap().unwrap();
    let inflight = wal.list_inflight().await.unwrap();
    assert!(
        inflight.is_empty(),
        "commit_async must remove the marker (got {} inflight)",
        inflight.len()
    );
}

#[tokio::test]
async fn commit_async_is_non_blocking_path() {
    // Confirms commit_async returns a JoinHandle synchronously
    // (i.e. it returns before the spawned task has a chance to
    // do anything). The caller is therefore free to ACK its
    // batch right after, without waiting on the marker remove.
    let wal = fresh();
    let txn_id = wal.fresh_txn_id();
    wal.begin(txn_id, vec![]).await.unwrap();
    let _handle: tokio::task::JoinHandle<DbResult<()>> = wal.commit_async(txn_id);
    // We don't await — the test asserts the synchronous return
    // shape only. The actual removal completes some time later;
    // it's verified in commit_async_eventually_removes_marker.
}

#[tokio::test]
async fn fresh_txn_ids_are_monotonic() {
    let wal = fresh();
    let a = wal.fresh_txn_id();
    let b = wal.fresh_txn_id();
    let c = wal.fresh_txn_id();
    assert!(b > a);
    assert!(c > b);
}

#[tokio::test]
async fn list_inflight_returns_mixed_v1_v2() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let wal = WalManager::new(Arc::clone(&store));

    // Write a V1 entry via existing begin().
    let txn_id_1 = wal.fresh_txn_id();
    wal.begin(
        txn_id_1,
        vec![WalOp::RecordCreated {
            record_id: RecordId::new(),
        }],
    )
    .await
    .unwrap();

    // Write a V2 entry manually (no public API to begin V2 yet —
    // comes in stage 4). Poke the bytes directly to simulate what
    // a future RepoWalManager would do.
    let v2 = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Delete {
            table_id_interned: 0,
            rid: RecordId::new(),
        }],
    );
    let v2_bytes = v2.encode().unwrap();
    let active_key = WalActiveKey::new(v2.txn_id).to_record_key();
    store.set(active_key, Bytes::from(v2_bytes)).await.unwrap();

    let listed = wal.list_inflight().await.unwrap();
    assert_eq!(listed.len(), 2);
    let mut v1_count = 0;
    let mut v2_count = 0;
    for entry in &listed {
        match entry {
            WalEntryAny::V1(_) => v1_count += 1,
            WalEntryAny::V2(_) => v2_count += 1,
        }
    }
    assert_eq!(v1_count, 1);
    assert_eq!(v2_count, 1);
}

#[tokio::test]
async fn list_inflight_v1_only_after_commit() {
    // Sanity: existing V1-only flow still works (regression guard).
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let wal = WalManager::new(Arc::clone(&store));

    let txn_id = wal.fresh_txn_id();
    wal.begin(
        txn_id,
        vec![WalOp::RecordCreated {
            record_id: RecordId::new(),
        }],
    )
    .await
    .unwrap();

    let listed = wal.list_inflight().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(matches!(listed[0], WalEntryAny::V1(_)));

    wal.commit(txn_id).await.unwrap();
    let listed = wal.list_inflight().await.unwrap();
    assert!(listed.is_empty());
}
