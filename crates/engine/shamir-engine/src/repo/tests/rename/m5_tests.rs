use crate::repo::{BoxRepo, RepoInstance};
use crate::table::{TableConfig, TableManager};
use crate::tx::commit::{commit_tx, TxError};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;
use std::time::Duration;

async fn fixture() -> (RepoInstance, TableManager) {
    let repo = RepoInstance::new(
        "m5_rename_test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("a").with_new_store_id()],
    );
    let table = repo.get_table("a").await.unwrap();
    (repo, table)
}

async fn stage_write(
    repo: &RepoInstance,
    table: &TableManager,
) -> (shamir_tx::TxContext, RecordId, shamir_tx::SnapshotGuard) {
    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let id = table
        .insert_tx(&InnerValue::Str("m5-value".into()), Some(&mut tx))
        .await
        .unwrap();
    (tx, id, guard)
}

#[tokio::test]
async fn staged_tx_is_rejected_after_successful_rename() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (repo, source) = fixture().await;
        let old = source.clone();
        let (tx, _, _snapshot_guard) = stage_write(&repo, &old).await;

        assert!(repo.rename_table_stores("a", "b").await.unwrap());
        assert!(matches!(
            commit_tx(tx, &repo).await,
            Err(TxError::MigrationFrozen { table }) if table == "a"
        ));
        assert!(!repo.has_table("a"));
        assert!(repo.has_table("b"));
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn staged_tx_is_rejected_after_drop_and_recreate() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (repo, source) = fixture().await;
        let (tx, _, _snapshot_guard) = stage_write(&repo, &source).await;

        let permit = repo.write_domain().admit().await.unwrap();
        assert!(repo.remove_table("a", &permit));
        repo.add_table(TableConfig::new("a").with_new_store_id(), &permit);
        drop(permit);
        let recreated = repo.get_table("a").await.unwrap();

        assert!(matches!(
            commit_tx(tx, &repo).await,
            Err(TxError::MigrationFrozen { table }) if table == "a"
        ));
        assert!(!Arc::ptr_eq(source.data_store(), recreated.data_store()));
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn staged_tx_for_current_incarnation_commits() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (repo, table) = fixture().await;
        let (tx, id, _snapshot_guard) = stage_write(&repo, &table).await;
        commit_tx(tx, &repo).await.unwrap();
        assert_eq!(
            table.get(id).await.unwrap(),
            InnerValue::Str("m5-value".into())
        );
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn staged_tx_for_dropped_table_is_rejected() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (repo, source) = fixture().await;
        let (tx, _, _snapshot_guard) = stage_write(&repo, &source).await;
        let permit = repo.write_domain().admit().await.unwrap();
        assert!(repo.remove_table("a", &permit));
        drop(permit);

        assert!(matches!(
            commit_tx(tx, &repo).await,
            Err(TxError::MigrationFrozen { table }) if table == "a"
        ));
    })
    .await
    .expect("test timed out");
}
