use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::{RecordKey, Repo};
use shamir_tx::changefeed::{encode_gap_marker, gap_marker_key};

use crate::repo::prepare_seed::prepare_seed;
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;

#[tokio::test]
async fn certified_through_above_visibility_refuses_seed_without_erasing_gap() {
    let backing = Arc::new(InMemoryRepo::new());
    let journal = backing.store_get("__changelog__").await.unwrap();
    let gap = encode_gap_marker((1, 10));
    journal
        .set(gap_marker_key().into(), gap.clone())
        .await
        .unwrap();
    let repo = RepoInstance::new("main".into(), BoxRepo::InMemory(backing), vec![]);
    assert_eq!(repo.consistent_version().await.unwrap(), 0);
    assert_eq!(repo.certified_gap_interval().await.unwrap(), Some((1, 10)));
    assert!(prepare_seed(&repo)
        .await
        .unwrap_err()
        .to_string()
        .contains("gap"));
    assert_eq!(journal.get(gap_marker_key().into()).await.unwrap(), gap);
}

#[tokio::test]
async fn corrupt_history_key_is_not_silently_skipped() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = RepoInstance::new(
        "main".into(),
        BoxRepo::InMemory(backing),
        vec![TableConfig::new("rows")],
    );
    let table = repo.get_table("rows").await.unwrap();
    let mvcc = table.mvcc_store().unwrap();
    mvcc.history_store()
        .set(RecordKey::from_slice(b"invalid-key"), vec![0x80].into())
        .await
        .unwrap();
    assert!(prepare_seed(&repo)
        .await
        .unwrap_err()
        .to_string()
        .contains("history key"));
}
