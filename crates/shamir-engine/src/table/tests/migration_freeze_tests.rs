//! Source-freeze regression tests: drain, ownership, and locked tx rejection.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use futures::poll;
use shamir_query_types::batch::BatchError;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;
use tokio::sync::Notify;

use crate::index::write_barrier_flags::{MIGRATION_FREEZE, SCHEMA_ACTIVATION};
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::{TableConfig, TableManager};
use crate::tx::commit::{PostValidatePrePublishHook, TEST_POST_VALIDATE_PRE_PUBLISH_HOOK};
use crate::tx::CommitError;

async fn fixture() -> (RepoInstance, TableManager, InnerValue) {
    let repo = RepoInstance::new(
        "freeze_test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("source"));
    let table = repo.get_table("source").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let key = match interner.touch_ind("name").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    let mut fields = new_map_wc(1);
    fields.insert(InternerKey::new(key), InnerValue::Str("Alice".into()));
    (repo, table, InnerValue::Map(fields))
}

/// R6: removing the drain makes the first poll return Ready while the row
/// is still unpublished; the frozen snapshot would omit the parked writer.
#[tokio::test]
async fn r6_freeze_waits_for_parked_fast_committer_and_observes_row() {
    let (repo, table, value) = fixture().await;
    assert!(!table.needs_write_barrier());
    let (mut tx, _snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    let hook = Arc::new(PostValidatePrePublishHook {
        reached: AtomicUsize::new(0),
        resume: Notify::new(),
        armed: AtomicBool::new(true),
    });
    TEST_POST_VALIDATE_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("nextest isolates each test's one-shot hook");
    let committer = repo.clone();
    let commit = tokio::spawn(async move { committer.commit_tx(tx).await });
    while hook.reached.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(table.count().await.unwrap(), 0);

    let mut freeze = Box::pin(table.begin_migration_freeze());
    let pending = poll!(freeze.as_mut()).is_pending();
    // Release and join even if the assertion below will fail.
    hook.resume.notify_one();
    let outcome = commit.await.unwrap().unwrap();
    assert!(
        pending,
        "freeze completed before the parked writer published"
    );
    assert!(outcome.materialized());
    let guard = freeze.await.unwrap();
    assert_eq!(table.count().await.unwrap(), 1);
    assert_eq!(table.get(rid).await.unwrap(), value);
    drop(guard);
    assert!(!table.needs_write_barrier());
}

/// R7: unconditional set would admit two owners; dropping the second would
/// clear the first's bit. The rejected attempt must not release that bit.
#[tokio::test]
async fn r7_second_freeze_rejected_without_clearing_first_owner() {
    let (_, table, _) = fixture().await;
    let guard = table.begin_migration_freeze().await.unwrap();
    let error = table.begin_migration_freeze().await.unwrap_err();
    assert!(
        matches!(&error, CommitError::MigrationFrozen { table } if table == "source"),
        "unexpected error: {error:?}",
    );
    assert!(table.needs_write_barrier());
    assert!(table.ddl_admission.try_lock().is_ok());
    drop(guard);
    assert!(!table.needs_write_barrier());
    let again = table.begin_migration_freeze().await.unwrap();
    drop(again);
    assert!(!table.needs_write_barrier());
}

/// R8: the unique-index slow committer has already queued for the lock when
/// freeze raises its bit. Checking only before lock acquisition misses it.
#[tokio::test]
async fn r8_unique_slow_committer_rechecks_freeze_under_lock() {
    let (repo, table, value) = fixture().await;
    table
        .create_unique_index("unique_name", &["name"])
        .await
        .unwrap();
    // Resolve lazy setup before the bounded poll probes the unique lock.
    repo.repo_wal().await.unwrap();
    let (mut tx, _snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    let lock = table.unique_write_lock().lock_owned().await;
    let mut commit = Box::pin(repo.commit_tx(tx));
    assert!(poll!(commit.as_mut()).is_pending());
    let mut freeze = Box::pin(table.begin_migration_freeze());
    assert!(poll!(freeze.as_mut()).is_pending());
    assert!(table
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    drop(lock);
    let error = commit.await.unwrap_err();
    assert!(
        matches!(&error, CommitError::MigrationFrozen { table } if table == "source"),
        "unexpected error: {error:?}",
    );
    let guard = freeze.await.unwrap();
    assert_eq!(table.count().await.unwrap(), 0);
    assert!(table.get(rid).await.is_err());
    drop(guard);
    let (mut tx, _snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    repo.commit_tx(tx).await.unwrap();
    assert_eq!(table.get(rid).await.unwrap(), value);
}

/// A cancelled preparation must synchronously release only its own bit,
/// whether parked in drain or at the final lock fence.
#[tokio::test]
async fn cancelled_freeze_clears_bit_at_both_preparation_awaits() {
    let (_, table, _) = fixture().await;
    let flags = table.index_manager_ref().write_barrier_flags();
    table.set_schema_activation_barrier(true);
    let writer = table.enter_writer_drain();
    let mut freeze = Box::pin(table.begin_migration_freeze());
    assert!(poll!(freeze.as_mut()).is_pending());
    assert!(flags.is_set(MIGRATION_FREEZE));
    drop(freeze);
    assert!(!flags.is_set(MIGRATION_FREEZE));
    assert!(flags.is_set(SCHEMA_ACTIVATION));
    drop(writer);
    table.set_schema_activation_barrier(false);

    let lock = table.unique_write_lock().lock_owned().await;
    let mut freeze = Box::pin(table.begin_migration_freeze());
    assert!(poll!(freeze.as_mut()).is_pending());
    assert!(flags.is_set(MIGRATION_FREEZE));
    drop(freeze);
    assert!(!flags.is_set(MIGRATION_FREEZE));
    assert!(table.ddl_admission.try_lock().is_ok());
    drop(lock);
    let guard = table.begin_migration_freeze().await.unwrap();
    drop(guard);
    assert!(!table.needs_write_barrier());
}

#[tokio::test]
async fn implicit_commit_maps_freeze_to_table_migrating() {
    let (repo, table, value) = fixture().await;
    let (mut tx, _snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    let guard = table.begin_migration_freeze().await.unwrap();
    let error = repo
        .commit_implicit_batch_tx(tx, "write")
        .await
        .unwrap_err();
    assert!(
        matches!(&error, BatchError::QueryError { alias, code: Some(code), .. }
            if alias == "write" && code == "table_migrating"),
        "unexpected wire mapping: {error:?}",
    );
    assert_eq!(table.count().await.unwrap(), 0);
    drop(guard);
}

/// R8 non-tx: each writer is queued on an existing unique table BEFORE
/// freeze raises its bit. A pre-lock-only check or any missing CRUD gate
/// allows a mutation instead of returning the typed migration error.
#[tokio::test]
async fn r8_non_tx_crud_queued_before_freeze_rejects_without_mutation() {
    for operation in ["insert", "insert_many", "delete", "set"] {
        let (_, table, original) = fixture().await;
        let rid = table.insert(&original).await.unwrap();
        table
            .create_unique_index("unique_name", &["name"])
            .await
            .unwrap();
        let mut changed = original.clone();
        if let InnerValue::Map(fields) = &mut changed {
            for (_, value) in fields.iter_mut() {
                *value = InnerValue::Str("Bob".into());
            }
        }
        let values = [changed.clone()];
        let lock = table.unique_write_lock().lock_owned().await;
        let mut write = Box::pin(async {
            match operation {
                "insert" => table.insert(&changed).await.map(|_| ()),
                "insert_many" => table.insert_many(&values).await.map(|_| ()),
                "delete" => table.delete(rid).await.map(|_| ()),
                "set" => table.set(rid, &changed).await.map(|_| ()),
                _ => unreachable!("closed operation list"),
            }
        });
        // In-memory setup is complete: this poll reaches the held mutex.
        assert!(poll!(write.as_mut()).is_pending(), "operation: {operation}");
        let mut freeze = Box::pin(table.begin_migration_freeze());
        assert!(poll!(freeze.as_mut()).is_pending());
        assert!(table
            .index_manager_ref()
            .write_barrier_flags()
            .is_set(MIGRATION_FREEZE));
        drop(lock);
        let error = write.await.unwrap_err();
        assert!(
            matches!(&error, DbError::MigrationFrozen { table } if table == "source"),
            "operation {operation}: {error:?}",
        );
        assert_eq!(error.code(), Some("table_migrating"));
        let guard = freeze.await.unwrap();
        assert_eq!(table.count().await.unwrap(), 1, "operation: {operation}");
        assert_eq!(table.get(rid).await.unwrap(), original);

        let interner = table.interner().get().await.unwrap();
        let index = interner.touch_ind("unique_name").unwrap().key().id();
        let posting = table
            .index_manager_ref()
            .lookup_by_unique_index(index, &[InnerValue::Str("Alice".into())])
            .await
            .unwrap();
        assert_eq!(posting, Some(rid), "operation: {operation}");
        let absent = table
            .index_manager_ref()
            .lookup_by_unique_index(index, &[InnerValue::Str("Bob".into())])
            .await
            .unwrap();
        assert_eq!(absent, None, "operation: {operation}");
        drop(guard);
        assert!(!table
            .index_manager_ref()
            .write_barrier_flags()
            .is_set(MIGRATION_FREEZE));
        table.set(rid, &changed).await.unwrap();
        assert_eq!(table.get(rid).await.unwrap(), changed);
    }
}

/// Conversion preserves typed freeze/storage errors and the existing conflict,
/// CAS and duplicate-key categories rather than erasing them into strings.
#[test]
fn tx_error_conversion_preserves_error_categories() {
    let frozen = DbError::from(CommitError::MigrationFrozen {
        table: "source".into(),
    });
    assert!(matches!(
        &frozen,
        DbError::MigrationFrozen { table } if table == "source"
    ));
    assert_eq!(frozen.code(), Some("table_migrating"));
    let storage = DbError::from(CommitError::Storage(DbError::IndexDrainInProgress(
        "index".into(),
    )));
    assert!(matches!(
        &storage,
        DbError::IndexDrainInProgress(index) if index == "index"
    ));
    assert_eq!(storage.code(), Some("index_drain_in_progress"));
    for error in [
        CommitError::SsiConflict { key: "key".into() },
        CommitError::PhantomConflict {
            dep: "predicate".into(),
        },
        CommitError::Wounded { tx_version: 7 },
    ] {
        let detail = error.to_string();
        let converted = DbError::from(error);
        assert!(matches!(&converted, DbError::Conflict(message) if message == &detail));
        assert_eq!(converted.code(), Some("tx_conflict"));
    }
    let cas = DbError::from(CommitError::CasConflict {
        key: "key".into(),
        expected: 1,
        found: 2,
    });
    assert!(matches!(&cas, DbError::VersionConflict(_)));
    assert_eq!(cas.code(), Some("version_conflict"));
    let unique = DbError::from(CommitError::UniqueViolation { key: "key".into() });
    assert!(matches!(&unique, DbError::DuplicateKey(_)));
    let expired = CommitError::Expired {
        elapsed: std::time::Duration::from_secs(2),
        max: std::time::Duration::from_secs(1),
    };
    let detail = expired.to_string();
    let expired = DbError::from(expired);
    assert!(matches!(&expired, DbError::Internal(message) if message == &detail));
    assert_eq!(expired.code(), None);
}
