use std::sync::Arc;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;

fn create_test_instance() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

fn make_repo() -> RepoInstance {
    create_test_instance()
}

#[tokio::test]
async fn tx_gate_initializes_lazy() {
    let instance = create_test_instance();
    let gate1 = instance.tx_gate().await.unwrap();
    let gate2 = instance.tx_gate().await.unwrap();
    assert!(
        Arc::ptr_eq(&gate1, &gate2),
        "tx_gate must return the same Arc on repeated calls"
    );
}

#[tokio::test]
async fn repo_wal_initializes_lazy() {
    let instance = create_test_instance();
    let wal1 = instance.repo_wal().await.unwrap();
    let wal2 = instance.repo_wal().await.unwrap();
    assert!(
        Arc::ptr_eq(&wal1, &wal2),
        "repo_wal must return the same Arc on repeated calls"
    );
}

#[tokio::test]
async fn tx_gate_and_wal_share_info_store_via_repo() {
    let instance = create_test_instance();
    let _gate = instance.tx_gate().await.unwrap();
    let _wal = instance.repo_wal().await.unwrap();

    let gate2 = instance.tx_gate().await.unwrap();
    let wal2 = instance.repo_wal().await.unwrap();
    let gate3 = instance.tx_gate().await.unwrap();
    let wal3 = instance.repo_wal().await.unwrap();

    assert!(Arc::ptr_eq(&gate2, &gate3));
    assert!(Arc::ptr_eq(&wal2, &wal3));
}

#[test]
fn repo_name_stored() {
    let repo = create_test_instance();
    assert_eq!(repo.name(), "test");
}

#[test]
fn repo_token_deterministic() {
    let t1 = crate::repo::repo_instance::repo_token("my_repo");
    let t2 = crate::repo::repo_instance::repo_token("my_repo");
    assert_eq!(t1, t2);
    assert_ne!(t1, 0);
}

#[tokio::test]
async fn create_table_context_populates_per_table_mvcc() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let _ = repo.get_table("users").await.unwrap();

    let token = crate::table::table_manager::table_token_for("users");

    let (tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    assert!(
        tx.version_provider.is_some(),
        "Serializable tx must have provider"
    );

    let v = tx
        .version_provider
        .as_ref()
        .unwrap()
        .version_of(token, &bytes::Bytes::from_static(b"unknown"));
    assert_eq!(v, 0, "unknown key returns version 0");
}

#[tokio::test]
async fn begin_tx_snapshot_does_not_attach_provider() {
    let repo = make_repo();
    let (tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    assert!(
        tx.version_provider.is_none(),
        "SI tx must not have provider"
    );
}
