//! V2 recovery tests (Stage 7.1.a skeleton + 7.1.c–d apply logic).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::{repo_token, BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

#[tokio::test]
async fn recover_v2_inflight_clean_repo_is_zero() {
    let repo = make_repo();
    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 0, "clean repo has no inflight entries");
}

#[tokio::test]
async fn recover_v2_inflight_replays_and_removes_entries() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 42;
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: 0,
            rid: RecordId(rid_bytes),
            body: bytes::Bytes::from_static(b"payload"),
        }],
    );
    wal.begin(entry).await.unwrap();

    let inflight = wal.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    let inflight = wal.list_inflight().await.unwrap();
    assert!(inflight.is_empty(), "marker must be cleaned after recovery");
}

#[tokio::test]
async fn recover_v2_inflight_handles_multiple_entries() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    for i in 0..3u64 {
        let entry = WalEntryV2::new(
            wal.fresh_txn_id(),
            0,
            vec![WalOpV2::CounterDelta {
                table_id_interned: i,
                delta: 1,
            }],
        );
        wal.begin(entry).await.unwrap();
    }

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 3);
    assert!(wal.list_inflight().await.unwrap().is_empty());
}

#[tokio::test]
async fn recover_v2_inflight_replays_put_applies_to_data_store() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 42;
    let rid = RecordId(rid_bytes);
    let token = table_token_for("t");

    let value = InnerValue::Str("recovered".into());
    let body = value.to_bytes().unwrap();

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid,
            body,
        }],
    );
    wal.begin(entry).await.unwrap();

    assert!(tbl.get(rid).await.is_err());

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    let read_back = tbl.get(rid).await.unwrap();
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "recovered"),
        "expected recovered Str, got {:?}",
        read_back
    );
}

#[tokio::test]
async fn recover_v2_inflight_replays_delete_removes_from_data_store() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let rid = tbl.insert(&InnerValue::Str("doomed".into())).await.unwrap();
    let _ = tbl.get(rid).await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Delete {
            table_id_interned: token,
            rid,
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    assert!(
        tbl.get(rid).await.is_err(),
        "rid should be gone after delete recovery"
    );
}

#[tokio::test]
async fn recover_v2_inflight_replays_counter_delta_increments() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let before = tbl.counter().get().await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::CounterDelta {
            table_id_interned: token,
            delta: 5,
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    let after = tbl.counter().get().await.unwrap();
    assert_eq!(after as i64 - before as i64, 5);
}

#[tokio::test]
async fn recover_v2_inflight_unknown_table_skips_gracefully() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();
    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 99;

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Put {
            table_id_interned: 99999,
            rid: RecordId(rid_bytes),
            body: bytes::Bytes::from_static(b"orphan"),
        }],
    );
    wal.begin(entry).await.unwrap();

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);
    assert!(wal.list_inflight().await.unwrap().is_empty());
}

#[tokio::test]
async fn recover_v2_inflight_replays_index_put_with_table_id() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let info = tbl.info_store().clone();

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");

    let key = bytes::Bytes::from_static(b"some_posting_key");
    let value = bytes::Bytes::from_static(b"some_posting_value");

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::IndexPut {
            table_id_interned: token,
            idx_id: 0,
            key: key.clone(),
            value: value.clone(),
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    let read_back = info.get(key).await.unwrap();
    assert_eq!(read_back, value);
}

#[tokio::test]
async fn recover_v2_inflight_replays_index_put_broadcast() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("a"));
    repo.add_table(TableConfig::new("b"));
    let ta = repo.get_table("a").await.unwrap();
    let tb = repo.get_table("b").await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let key = bytes::Bytes::from_static(b"broadcast_key");
    let value = bytes::Bytes::from_static(b"broadcast_val");

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::IndexPut {
            table_id_interned: 0,
            idx_id: 0,
            key: key.clone(),
            value: value.clone(),
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    assert_eq!(ta.info_store().get(key.clone()).await.unwrap(), value);
    assert_eq!(tb.info_store().get(key).await.unwrap(), value);
}

#[tokio::test]
async fn recover_v2_inflight_replays_index_del() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let info = tbl.info_store().clone();

    let key = bytes::Bytes::from_static(b"doomed");
    info.set(key.clone(), bytes::Bytes::from_static(b"val"))
        .await
        .unwrap();
    assert!(info.get(key.clone()).await.is_ok());

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::IndexDel {
            table_id_interned: token,
            idx_id: 0,
            key: key.clone(),
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();
    assert!(info.get(key).await.is_err(), "key removed by IndexDel");
}
