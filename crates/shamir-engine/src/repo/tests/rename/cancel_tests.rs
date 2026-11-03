use crate::index::write_barrier_flags::MIGRATION_FREEZE;
use crate::repo::{BoxRepo, RenamePersistOutcome, RepoInstance};
use crate::table::{TableConfig, TableManager};
use futures::poll;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;

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
async fn cancelled_persisting_rename_keeps_source_fenced() {
    let (repo, source) = fixture().await;
    repo.tx_gate().await.unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&entered);
    let mut rename = Box::pin(
        repo.rename_table_stores_with("a", "b", move |_| async move {
            flag.store(true, Ordering::Release);
            std::future::pending::<RenamePersistOutcome>().await
        }),
    );
    for _ in 0..100 {
        match poll!(rename.as_mut()) {
            Poll::Ready(result) => panic!("rename completed early: {result:?}"),
            Poll::Pending if entered.load(Ordering::Acquire) => break,
            Poll::Pending => tokio::task::yield_now().await,
        }
    }
    assert!(entered.load(Ordering::Acquire));
    drop(rename);
    assert!(repo.has_table("a"));
    assert!(!repo.has_table("b"));
    assert!(source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    assert!(repo.has_rename_in_doubt());
    assert!(matches!(
        repo.get_table("a").await,
        Err(DbError::Conflict(_))
    ));
    assert!(matches!(
        repo.get_table("b").await,
        Err(DbError::Conflict(_))
    ));
    assert!(matches!(
        source.create_index("while_cancelled", &["name"]).await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));
    assert!(matches!(
        repo.rename_table_stores_with("a", "b", |_| async {
            RenamePersistOutcome::InDoubt(DbError::Storage("cancelled while persisting".into()))
        })
        .await,
        Err(DbError::Conflict(prefix)) if prefix.starts_with("rename")
    ));
    assert!(matches!(
        source.insert(&InnerValue::Null).await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));

    assert!(repo
        .rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed })
        .await
        .unwrap());
    assert!(!repo.has_rename_in_doubt());
    assert!(!repo.has_table("a"));
    assert!(repo.has_table("b"));
}

#[tokio::test]
async fn fenced_retry_cancel_keeps_both_names_fenced() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (repo, source) = fixture().await;
        let row_id = source
            .insert(&InnerValue::Str("preserved".into()))
            .await
            .unwrap();
        assert!(matches!(
            repo.rename_table_stores_with("a", "b", |_| async {
                RenamePersistOutcome::InDoubt(DbError::Storage("uncertain".into()))
            })
            .await,
            Err(DbError::Conflict(prefix)) if prefix.starts_with("rename")
        ));

        let entered = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&entered);
        let mut retry = Box::pin(
            repo.rename_table_stores_with("a", "b", move |_| async move {
                flag.store(true, Ordering::Release);
                std::future::pending::<RenamePersistOutcome>().await
            }),
        );
        let mut saw_entered = false;
        for _ in 0..100 {
            match poll!(retry.as_mut()) {
                Poll::Ready(result) => panic!("retry completed early: {result:?}"),
                Poll::Pending if entered.load(Ordering::Acquire) => {
                    saw_entered = true;
                    break;
                }
                Poll::Pending => tokio::task::yield_now().await,
            }
        }
        assert!(saw_entered, "retry persistence callback was not entered");
        assert!(repo.has_rename_in_doubt());
        assert!(repo.check_table_rename_fence("a").is_err());
        assert!(repo.check_table_rename_fence("b").is_err());
        assert!(matches!(
            repo.get_table("a").await,
            Err(DbError::Conflict(_))
        ));
        assert!(matches!(
            repo.get_table("b").await,
            Err(DbError::Conflict(_))
        ));
        assert!(matches!(
            source.insert(&InnerValue::Null).await,
            Err(DbError::MigrationFrozen { table }) if table == "a"
        ));
        assert!(matches!(
            source.create_index("during_retry", &["name"]).await,
            Err(DbError::MigrationFrozen { table }) if table == "a"
        ));
        drop(retry);

        assert!(repo.has_rename_in_doubt());
        assert!(repo.check_table_rename_fence("a").is_err());
        assert!(repo.check_table_rename_fence("b").is_err());
        assert!(matches!(
            repo.get_table("a").await,
            Err(DbError::Conflict(_))
        ));
        assert!(matches!(
            repo.get_table("b").await,
            Err(DbError::Conflict(_))
        ));
        assert!(matches!(
            source.insert(&InnerValue::Null).await,
            Err(DbError::MigrationFrozen { table }) if table == "a"
        ));
        assert!(matches!(
            source.create_index("after_cancel", &["name"]).await,
            Err(DbError::MigrationFrozen { table }) if table == "a"
        ));

        assert!(repo
            .rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed })
            .await
            .unwrap());
        assert!(!repo.has_rename_in_doubt());
        let destination = repo.get_table("b").await.unwrap();
        assert_eq!(
            destination.get(row_id).await.unwrap(),
            InnerValue::Str("preserved".into())
        );
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn cancelled_visibility_wait_reattaches_and_unfreezes_source() {
    let (repo, source) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    let version = gate.assign_next_version_guarded();
    let mut rename = Box::pin(
        repo.rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed }),
    );
    for _ in 0..100 {
        match poll!(rename.as_mut()) {
            Poll::Ready(result) => panic!("rename completed early: {result:?}"),
            Poll::Pending
                if source
                    .index_manager_ref()
                    .write_barrier_flags()
                    .is_set(MIGRATION_FREEZE) =>
            {
                break
            }
            Poll::Pending => tokio::task::yield_now().await,
        }
    }
    assert!(source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    drop(rename);
    drop(version);
    assert_source_recovered(&repo, &source).await;
}
