use crate::index::write_barrier_flags::MIGRATION_FREEZE;
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::{TableConfig, TableManager};
use futures::poll;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

async fn fixture() -> (RepoInstance, TableManager) {
    let repo = RepoInstance::new(
        "rename_cancel_test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("a")],
    );
    let source = repo.get_table("a").await.unwrap();
    (repo, source)
}

async fn assert_source_recovered(repo: &RepoInstance, source: &TableManager) {
    assert!(repo.has_table("a"));
    assert!(!repo.has_table("b"));
    assert!(!source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    source.ensure_attached().unwrap();
    source.insert(&InnerValue::Null).await.unwrap();
    source
        .create_index("after_cancel", &["name"])
        .await
        .unwrap();
}

#[tokio::test]
async fn cancelled_persist_reattaches_and_unfreezes_source() {
    let (repo, source) = fixture().await;
    repo.tx_gate().await.unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&entered);
    let mut rename = Box::pin(
        repo.rename_table_stores_with("a", "b", move |_| async move {
            flag.store(true, Ordering::Release);
            std::future::pending::<Result<(), DbError>>().await
        }),
    );
    for _ in 0..100 {
        if poll!(rename.as_mut()).is_pending() && entered.load(Ordering::Acquire) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(entered.load(Ordering::Acquire));
    drop(rename);
    assert_source_recovered(&repo, &source).await;
}

#[tokio::test]
async fn cancelled_visibility_wait_reattaches_and_unfreezes_source() {
    let (repo, source) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    let version = gate.assign_next_version_guarded();
    let mut rename = Box::pin(repo.rename_table_stores_with("a", "b", |_| async { Ok(()) }));
    for _ in 0..100 {
        if poll!(rename.as_mut()).is_pending()
            && source
                .index_manager_ref()
                .write_barrier_flags()
                .is_set(MIGRATION_FREEZE)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    drop(rename);
    drop(version);
    assert_source_recovered(&repo, &source).await;
}
