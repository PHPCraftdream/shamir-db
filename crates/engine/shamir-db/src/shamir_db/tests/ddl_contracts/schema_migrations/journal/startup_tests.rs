use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use shamir_storage::types::RecordKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use super::helpers::{
    body, checksum, context, find_journal_row, journal_table, on_big_stack, persistent_db,
    predecessor, prepare_database, run_apply, run_status, str_field, submission,
    wal_patch_journal_row,
};
use crate::engine::table::record_cow::RecordCow;
use crate::shamir_db::system_store::{
    DatabaseIdentity, JournalEvent, JournalEventKind, SchemaMigrationStep, SystemStoreConfig,
};
use crate::{DbError, ShamirDb};

const BUDGET: Duration = Duration::from_secs(120);

async fn reopen(path: &Path) -> Result<ShamirDb, DbError> {
    tokio::time::timeout(BUDGET, async {
        loop {
            match Box::pin(ShamirDb::init(SystemStoreConfig::Fjall(
                path.join("system"),
            )))
            .await
            {
                // FjallRepo::new erases the original variant into Storage(String).
                Err(DbError::Storage(detail)) if detail == "FjallError: Locked" => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                result => return result,
            }
        }
    })
    .await
    .expect("reopen exceeded its lock-retry budget")
}

async fn close(db: ShamirDb) {
    db.flush_all_waiting().await.expect("clean engine shutdown");
}

async fn started() -> (tempfile::TempDir, ShamirDb, DatabaseIdentity) {
    let (dir, db) = persistent_db().await;
    db.create_db("alpha").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("alpha")
        .await
        .unwrap();
    let migration = body("alpha-001", 1);
    db.system_store()
        .acquire_schema_migration_fence(
            identity,
            0,
            migration.metadata.clone(),
            migration,
            false,
            &context(),
        )
        .await
        .unwrap();
    (dir, db, identity)
}

async fn assert_open_damage(path: &Path, diagnostics: &[&str]) {
    let Err(error) = reopen(path).await else {
        panic!("damaged journal must prevent startup: {diagnostics:?}");
    };
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable")
    );
    assert!(
        matches!(error, DbError::SchemaMigrationJournalUnreadable(ref detail)
            if diagnostics.iter().all(|&diagnostic| detail.contains(diagnostic))),
        "unexpected startup error: {error}"
    );
}

#[test]
fn duplicate_journal_record_key_during_open_rebuild_is_typed() {
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(async {
                let (dir, db, _) = started().await;
                let table = journal_table(&db).await;
                assert!(
                    table
                        .unique_index_exists("schema_migration_record_key")
                        .await
                );
                let raw = {
                    let mut stream = Box::pin(table.list_stream(16));
                    let mut batch = stream.next().await.unwrap().unwrap();
                    let (_, RecordCow::Borrowed(raw)) = batch.pop().unwrap() else {
                        panic!("journal MVCC scan must yield raw record bytes");
                    };
                    raw
                };
                // Bypass the unique write check, but write the same MVCC history
                // store the on-open rebuild reads. Keep the index definition.
                table
                    .mvcc_store_for_test()
                    .unwrap()
                    .set_versioned(RecordKey::from_slice(&RecordId::new().to_bytes()), raw)
                    .await
                    .unwrap();
                assert_eq!(super::helpers::rows(&table).await.len(), 3);
                shamir_index::persistence::clear_identity_markers(&table.info_store_for_test())
                    .await
                    .unwrap();
                db.system_store()
                    .flush_system_repo_durable(&table)
                    .await
                    .unwrap();
                drop(table);
                close(db).await;
                assert_open_damage(dir.path(), &["unique index could not be created on open"])
                    .await;
            }),
        )
        .await
        .expect("startup rebuild scenario timed out");
    });
}

async fn malformed_event_reopen(trailing: bool) {
    let (dir, db, identity) = started().await;
    let (_, key, row) = find_journal_row(&db, &identity, "event").await;
    let Some(QueryValue::Bin(raw)) = row.get("event") else {
        panic!("started row must carry a binary event");
    };
    let mut event: JournalEvent = rmp_serde::from_slice(raw).unwrap();
    if !trailing {
        event.database_identity = DatabaseIdentity::new();
    }
    let mut bytes = rmp_serde::to_vec_named(&event).unwrap();
    let diagnostic = if trailing {
        bytes.push(0xc0);
        "trailing bytes after event payload"
    } else {
        // The identity field is intact; the final fence_generation is not.
        bytes.pop().unwrap();
        assert!(rmp_serde::from_slice::<JournalEvent>(&bytes).is_err());
        "event decode failed"
    };
    let mut patch = new_map();
    patch.insert("event".into(), QueryValue::Bin(bytes));
    wal_patch_journal_row(&db, &key, QueryValue::Map(patch)).await;
    close(db).await;
    assert_open_damage(dir.path(), &["startup routing:", diagnostic]).await;
}

#[test]
fn truncated_event_after_foreign_identity_fails_whole_open() {
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(malformed_event_reopen(false)),
        )
        .await
        .expect("truncated event scenario timed out");
    });
}

#[test]
fn trailing_bytes_after_event_fail_whole_open() {
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(malformed_event_reopen(true)),
        )
        .await
        .expect("trailing event bytes scenario timed out");
    });
}

#[test]
fn applied_tail_seeds_fence_without_entering_operation_recovery() {
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(async {
                let (dir, db, identity) = started().await;
                prepare_database(&db, "alpha").await;
                let migration = body("alpha-001", 1);
                let digest = checksum(&migration);
                // Ledger-only fixture: exercise release of already-proven work,
                // not execution of its DDL. The retry must append only a release.
                for (kind, operation_index, evidence) in [
                    (JournalEventKind::OperationIntent, Some(0), None),
                    (
                        JournalEventKind::OperationComplete,
                        Some(0),
                        Some("completed"),
                    ),
                    (JournalEventKind::Applied, None, Some("verified")),
                ] {
                    db.system_store()
                        .append_schema_migration_event(
                            identity,
                            1,
                            migration.metadata.clone(),
                            digest,
                            SchemaMigrationStep {
                                kind,
                                operation_index,
                                evidence: evidence.map(str::to_owned),
                            },
                            &context(),
                        )
                        .await
                        .unwrap();
                }
                close(db).await;
                let db = reopen(dir.path()).await.unwrap();
                let store = db.system_store();
                let history = store.read_schema_migration_history(identity).await.unwrap();
                assert_eq!(history.applied.len(), 1);
                assert_eq!(history.events.last(), history.applied.first());
                assert!(history.unresolved.is_empty());
                assert_eq!(
                    store.owned_schema_migration_fence(identity).await.unwrap(),
                    Some(1)
                );
                assert_eq!(
                    db.schema_migration_refusal_code(identity),
                    Some("schema_migration_in_doubt")
                );
                let status = run_status(&db, "alpha", &identity).await.unwrap();
                assert_eq!(str_field(&status, "last_applied_id"), "alpha-001");
                assert_eq!(str_field(&status, "error"), "schema_migration_in_doubt");
                let in_flight = status.get("in_flight").expect("fence still owned");
                assert_eq!(str_field(in_flight, "status"), "applied");
                assert_eq!(
                    in_flight
                        .get("fence_generation")
                        .and_then(QueryValue::as_i64),
                    Some(1)
                );

                let result = run_apply(
                    &db,
                    "alpha",
                    predecessor(None, 0, [0; 32]),
                    1,
                    submission(&migration),
                    &identity,
                )
                .await
                .unwrap();
                assert_eq!(str_field(&result, "status"), "applied");
                assert_eq!(db.schema_migration_refusal_code(identity), None);
                let released = store.read_schema_migration_history(identity).await.unwrap();
                assert_eq!(released.applied, history.applied);
                assert_eq!(released.events.len(), history.events.len() + 1);
                assert_eq!(
                    &released.events[..history.events.len()],
                    history.events.as_slice()
                );
                let release = released.events.last().unwrap();
                assert_eq!(release.kind, JournalEventKind::FenceReleased);
                assert_eq!(release.fence_generation, 2);
                close(db).await;
                let db = reopen(dir.path()).await.unwrap();
                let history = db
                    .system_store()
                    .read_schema_migration_history(identity)
                    .await
                    .unwrap();
                assert_eq!(history.applied.len(), 1);
                assert_eq!(history.events.len(), 5);
                assert_eq!(history.events[4].kind, JournalEventKind::FenceReleased);
                assert_eq!(db.schema_migration_refusal_code(identity), None);
                close(db).await;
            }),
        )
        .await
        .expect("applied-tail scenario timed out");
    });
}
