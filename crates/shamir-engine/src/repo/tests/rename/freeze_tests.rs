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
        "rename_freeze_test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("a")],
    );
    let source = repo.get_table("a").await.unwrap();
    (repo, source)
}

#[tokio::test]
async fn successful_rename_rejects_stale_clone_crud_and_ddl() {
    let (repo, source) = tokio::time::timeout(std::time::Duration::from_secs(30), fixture())
        .await
        .expect("fixture timed out");
    source.create_index("ix_before", &["name"]).await.unwrap();
    let old = source.clone();

    assert!(repo
        .rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed })
        .await
        .unwrap());

    assert!(matches!(
        old.create_index("ix_after", &["name"]).await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));
    assert!(matches!(
        old.begin_write_barrier(crate::index::write_barrier_flags::SCHEMA_ACTIVATION)
            .await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));
    assert!(matches!(
        old.insert(&InnerValue::Null).await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));
    let permit = repo.write_domain().admit().await.unwrap();
    assert!(matches!(
        old.set_buffer_config_admitted(
            &shamir_storage::storage_membuffer::MemBufferConfig {
                max_bytes: 1024 * 1024,
                max_entries: 1000,
                ttl_ms: None,
                flush_interval_ms: 100,
                flush_batch_size: 100,
            },
            &permit,
        )
        .await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));
    drop(permit);

    let destination = tokio::time::timeout(std::time::Duration::from_secs(30), repo.get_table("b"))
        .await
        .expect("destination lookup timed out")
        .unwrap();
    let before = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        destination.index_exists("ix_before"),
    )
    .await
    .expect("index lookup timed out");
    let after = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        destination.index_exists("ix_after"),
    )
    .await
    .expect("index lookup timed out");
    assert!(before);
    assert!(!after);
    assert!(source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
}

#[tokio::test]
async fn in_doubt_rename_fences_both_names_until_same_pair_commits() {
    let (repo, source) = tokio::time::timeout(std::time::Duration::from_secs(30), fixture())
        .await
        .expect("fixture timed out");
    let old = source.clone();
    let row_id = source
        .insert(&InnerValue::Str("preserved".into()))
        .await
        .unwrap();
    assert!(matches!(
        repo.rename_table_stores_with("a", "b", |_| async {
            RenamePersistOutcome::InDoubt(DbError::Storage("uncertain".into()))
        })
        .await,
        Err(DbError::Conflict(_))
    ));
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
    assert_eq!(source.table_token(), old.table_token());
    assert!(matches!(
        old.insert(&InnerValue::Null).await,
        Err(DbError::MigrationFrozen { table }) if table == "a"
    ));
    assert!(matches!(
        repo.rename_table_stores_with("a", "c", |_| async { RenamePersistOutcome::Committed })
            .await,
        Err(DbError::Conflict(_))
    ));
    assert!(repo
        .rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed })
        .await
        .unwrap());
    assert!(!repo.has_rename_in_doubt());
    assert!(repo.has_table("b"));
    assert!(!repo.has_table("a"));
    let destination = repo.get_table("b").await.unwrap();
    assert_eq!(
        destination.get(row_id).await.unwrap(),
        InnerValue::Str("preserved".into())
    );
}

#[tokio::test]
async fn fenced_retry_not_committed_remains_fenced_until_commit() {
    let (repo, _) = tokio::time::timeout(std::time::Duration::from_secs(30), fixture())
        .await
        .expect("fixture timed out");
    let _ = repo
        .rename_table_stores_with("a", "b", |_| async {
            RenamePersistOutcome::InDoubt(DbError::Storage("uncertain".into()))
        })
        .await;
    assert!(matches!(
        repo.rename_table_stores_with("a", "b", |_| async {
            RenamePersistOutcome::NotCommitted(DbError::Storage("not committed".into()))
        })
        .await,
        Err(DbError::Conflict(message)) if message.starts_with("rename in doubt") && message.contains("not committed")
    ));
    assert!(repo.has_rename_in_doubt());
    assert!(repo.check_table_rename_fence("a").is_err());
    assert!(repo.check_table_rename_fence("b").is_err());
    assert!(repo
        .rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed })
        .await
        .unwrap());
    assert!(!repo.has_rename_in_doubt());
}

#[tokio::test]
async fn failed_rename_reattaches_and_unfreezes_source() {
    let (repo, source) = tokio::time::timeout(std::time::Duration::from_secs(30), fixture())
        .await
        .expect("fixture timed out");
    let old = source.clone();
    let error = repo
        .rename_table_stores_with("a", "b", |_| async {
            RenamePersistOutcome::NotCommitted(DbError::Storage("injected".into()))
        })
        .await
        .unwrap_err();
    assert!(matches!(&error, DbError::Storage(message) if message == "injected"));
    assert!(repo.has_table("a"));
    assert!(!repo.has_table("b"));
    assert!(!source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    old.insert(&InnerValue::Null).await.unwrap();
    old.create_index("ix_after_abort", &["name"]).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_is_drained_before_successful_rename() {
    let (repo, source) = tokio::time::timeout(std::time::Duration::from_secs(30), fixture())
        .await
        .expect("fixture timed out");
    let old = source.clone();
    let (rejected_tx, rejected_rx) = tokio::sync::oneshot::channel();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let mut writer = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(30), async move {
            let mut rejected_tx = Some(rejected_tx);
            let mut start_tx = Some(start_tx);
            let mut written = Vec::new();
            loop {
                match old
                    .insert(&InnerValue::Str(format!("row-{}", written.len())))
                    .await
                {
                    Ok(id) => {
                        written.push(id);
                        if let Some(sender) = start_tx.take() {
                            let _ = sender.send(());
                        }
                    }
                    Err(error) => {
                        if let Some(sender) = rejected_tx.take() {
                            let _ = sender.send(());
                        }
                        return (written, error);
                    }
                }
            }
        })
        .await
    });
    let start = tokio::time::timeout(std::time::Duration::from_secs(30), start_rx).await;
    if !matches!(start, Ok(Ok(()))) {
        writer.abort();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), &mut writer).await;
        panic!("writer did not start successfully: {start:?}");
    }
    let rename_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        repo.rename_table_stores_with("a", "b", |_| async move {
            match tokio::time::timeout(std::time::Duration::from_secs(30), rejected_rx).await {
                Ok(Ok(())) => RenamePersistOutcome::Committed,
                Ok(Err(_)) => RenamePersistOutcome::InDoubt(DbError::Internal(
                    "writer task dropped rejection signal".into(),
                )),
                Err(_) => RenamePersistOutcome::InDoubt(DbError::Conflict(
                    "writer rejection timed out".into(),
                )),
            }
        }),
    )
    .await;
    if !matches!(rename_result, Ok(Ok(true))) {
        writer.abort();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), &mut writer).await;
        panic!("rename failed or timed out: {rename_result:?}");
    }
    let joined = tokio::time::timeout(std::time::Duration::from_secs(30), &mut writer).await;
    let (ids, error) = match joined {
        Ok(Ok(Ok(result))) => result,
        other => {
            writer.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(30), &mut writer).await;
            panic!("writer did not finish successfully: {other:?}");
        }
    };
    assert!(!ids.is_empty());
    let expected_count = ids.len();
    assert!(matches!(&error, DbError::MigrationFrozen { table } if table == "a"));
    let destination = tokio::time::timeout(std::time::Duration::from_secs(30), repo.get_table("b"))
        .await
        .expect("destination lookup timed out")
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(30), destination.count())
            .await
            .expect("destination count timed out")
            .unwrap(),
        expected_count
    );
    for (index, id) in ids.into_iter().enumerate() {
        let value = tokio::time::timeout(std::time::Duration::from_secs(30), destination.get(id))
            .await
            .expect("destination read timed out")
            .unwrap();
        assert_eq!(value, InnerValue::Str(format!("row-{index}")));
    }
    let stale_write = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        source.insert(&InnerValue::Null),
    )
    .await
    .expect("stale-handle write timed out");
    assert!(matches!(stale_write, Err(DbError::MigrationFrozen { table }) if table == "a"));
}

#[tokio::test]
async fn rename_flushes_live_record_counter() {
    let (repo, source) = tokio::time::timeout(std::time::Duration::from_secs(30), fixture())
        .await
        .expect("fixture timed out");
    for i in 0..4 {
        source
            .insert(&InnerValue::Str(format!("row-{i}")))
            .await
            .unwrap();
    }
    assert!(repo
        .rename_table_stores_with("a", "b", |_| async { RenamePersistOutcome::Committed })
        .await
        .unwrap());
    let destination = tokio::time::timeout(std::time::Duration::from_secs(30), repo.get_table("b"))
        .await
        .expect("destination lookup timed out")
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(30), destination.count())
            .await
            .expect("destination count timed out")
            .unwrap(),
        4
    );
}

#[tokio::test]
async fn cancelled_preparing_rename_aborts_fresh_and_restores_source() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let (repo, source) = fixture().await;
        source.create_index("ix_prepare", &["name"]).await.unwrap();
        // Hold visibility so the rename stays in the preparing phase
        // (freeze installed, waiting for all pre-rename versions to settle)
        // and never reaches the persist callback.
        let gate = repo.tx_gate().await.unwrap();
        let version = gate.assign_next_version_guarded();
        let entered = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&entered);
        let mut rename = Box::pin(
            repo.rename_table_stores_with("a", "b", move |_| async move {
                flag.store(true, Ordering::Release);
                RenamePersistOutcome::NotCommitted(DbError::Storage("must not run".into()))
            }),
        );
        // Bounded poll until the source freeze is installed; panic if the
        // preparing phase never starts.
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
        assert!(
            !entered.load(Ordering::Acquire),
            "persist callback must not run during the preparing phase"
        );

        // Cancellation of a fresh (not yet fenced) attempt aborts the freeze.
        drop(rename);
        drop(version);

        assert!(repo.has_table("a"));
        assert!(!repo.has_table("b"));
        assert_eq!(repo.list_table_names(), vec!["a".to_string()]);
        assert!(!source
            .index_manager_ref()
            .write_barrier_flags()
            .is_set(MIGRATION_FREEZE));
        assert!(!repo.has_rename_in_doubt());
        assert!(repo.check_table_rename_fence("a").is_ok());
        assert!(repo.check_table_rename_fence("b").is_ok());
        assert!(source.index_exists("ix_prepare").await);
        source.insert(&InnerValue::Null).await.unwrap();
        source
            .create_index("ix_after_preparing_abort", &["name"])
            .await
            .unwrap();
    })
    .await
    .expect("test timed out");
}
