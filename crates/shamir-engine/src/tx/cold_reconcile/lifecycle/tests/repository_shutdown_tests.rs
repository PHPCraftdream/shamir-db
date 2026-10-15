use std::sync::Arc;
use std::time::Duration;

use shamir_query_builder::write::doc;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Repo;
use shamir_tx::changefeed::{clean_horizon_key, decode_clean_horizon};
use shamir_types::codecs::interned::query_value_to_inner;

use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;
use crate::tx::IsolationLevel;

#[tokio::test(start_paused = true)]
async fn held_commit_tail_makes_repository_shutdown_report_quiescence_timeout() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = RepoInstance::new(
        "repository-quiescence-timeout-test".into(),
        BoxRepo::InMemory(backing),
        vec![TableConfig::new("items")],
    );
    let _tail = repo.commit_tail().enter();

    let err = repo.shutdown_clean(Duration::ZERO).await.unwrap_err();
    assert!(err.to_string().contains("commit quiescence"));
    assert!(repo.worker_reopen_required());
}

#[tokio::test(start_paused = true)]
async fn never_installed_worker_flushes_and_checkpoints_committed_horizon() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = RepoInstance::new(
        "repository-shutdown-test".into(),
        BoxRepo::InMemory(Arc::clone(&backing)),
        vec![TableConfig::new("items")],
    );
    let table = repo.get_table("items").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let value = query_value_to_inner(&doc().set("name", "value").build(), interner).unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(snapshot);
    let horizon = repo
        .tx_gate()
        .await
        .unwrap()
        .version_allocation_high_water_mark();
    assert!(horizon > 0);

    repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();

    let store = backing.store_get("__changelog__").await.unwrap();
    let encoded = store.get(clean_horizon_key().into()).await.unwrap();
    assert_eq!(decode_clean_horizon(&encoded).unwrap(), horizon);
    assert!(!repo.worker_reopen_required());
    repo.changefeed()
        .await
        .unwrap()
        .feed
        .shutdown_and_wait()
        .await
        .unwrap();
}
