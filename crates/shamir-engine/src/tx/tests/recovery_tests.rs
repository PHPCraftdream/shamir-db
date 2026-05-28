//! V2 recovery skeleton tests (Stage 7.1.a).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::{repo_token, BoxRepo, RepoInstance};

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
