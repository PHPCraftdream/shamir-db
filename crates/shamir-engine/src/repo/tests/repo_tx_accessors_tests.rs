use std::sync::Arc;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use shamir_storage::storage_in_memory::InMemoryRepo;

fn create_test_instance() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(BoxRepo::InMemory(repo), Vec::new())
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
