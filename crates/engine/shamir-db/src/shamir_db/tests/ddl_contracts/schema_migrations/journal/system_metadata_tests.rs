use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use shamir_storage::types::RecordKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use super::helpers::{
    body, checksum, context, field, find_journal_row, journal_table, on_big_stack, persistent_db,
    predecessor, prepare_database, rows, run_apply, run_plan, run_status, str_field, submission,
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
        Some("schema_migration_durability_unavailable"),
        "unexpected startup error: {error:?}; expected diagnostics: {diagnostics:?}"
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
fn corrupt_journal_record_during_open_rebuild_is_typed() {
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(async {
                let (dir, db, _) = started().await;
                let table = journal_table(&db).await;
                // Undecodable raw MVCC row: a valid record-id key whose value
                // bytes are not a record (msgpack 0xc1 is never valid). The
                // open-time rebuild scan must classify this as journal damage
                // through the typed table context, not as a bare Codec.
                table
                    .mvcc_store_for_test()
                    .unwrap()
                    .set_versioned(
                        RecordKey::from_slice(&RecordId::new().to_bytes()),
                        bytes::Bytes::from_static(&[0xc1, 0x00]),
                    )
                    .await
                    .unwrap();
                shamir_index::persistence::clear_identity_markers(&table.info_store_for_test())
                    .await
                    .unwrap();
                db.system_store()
                    .flush_system_repo_durable(&table)
                    .await
                    .unwrap();
                drop(table);
                close(db).await;
                assert_open_damage(dir.path(), &["could not be decoded"]).await;
            }),
        )
        .await
        .expect("corrupt journal record scenario timed out");
    });
}

#[test]
fn unrelated_system_table_duplicate_rebuild_keeps_its_own_error() {
    on_big_stack(|| async {
        tokio::time::timeout(
            Duration::from_secs(300),
            Box::pin(async {
                let (dir, db, _) = started().await;
                // A trusted embedder installs a unique index on a NON-journal
                // system table and a duplicate row slips in. Its rebuild
                // failure must keep its own diagnostics: never journal damage.
                let table = db
                    .system_store()
                    .system_repo()
                    .expect("system repo is open")
                    .get_table("databases")
                    .await
                    .expect("databases table opens");
                table
                    .create_unique_index("databases_name_key", &["name"])
                    .await
                    .expect("unique index on a duplicate-free table is created");
                let raw = {
                    let mut stream = Box::pin(table.list_stream(16));
                    let mut batch = stream.next().await.unwrap().unwrap();
                    let (_, RecordCow::Borrowed(raw)) = batch.pop().unwrap() else {
                        panic!("databases MVCC scan must yield raw record bytes");
                    };
                    raw
                };
                table
                    .mvcc_store_for_test()
                    .unwrap()
                    .set_versioned(RecordKey::from_slice(&RecordId::new().to_bytes()), raw)
                    .await
                    .unwrap();
                // Pre-close control: exactly the original row plus the raw
                // duplicate (created by `db.create_db("alpha")`, whose retained
                // WAL entry opens this table inside cold replay).
                assert_eq!(rows(&table).await.len(), 2);
                shamir_index::persistence::clear_identity_markers(&table.info_store_for_test())
                    .await
                    .unwrap();
                db.system_store()
                    .flush_system_repo_durable(&table)
                    .await
                    .unwrap();
                drop(table);
                close(db).await;
                let Err(error) = reopen(dir.path()).await else {
                    panic!("duplicate rebuild must still fail the open");
                };
                assert_ne!(
                    error.code(),
                    Some("schema_migration_durability_unavailable"),
                    "a non-journal rebuild failure must not be journal damage: {error}"
                );
                // The typed rebuild context names the failing table and keeps
                // the original diagnostics.
                let DbError::IndexRebuildFailed { table, source } = &error else {
                    panic!("unexpected startup error: {error:?}");
                };
                assert_eq!(
                    table, "databases",
                    "rebuild context names the failing table"
                );
                assert!(
                    matches!(source.as_ref(), DbError::UniqueIndexCreationFailed(..)),
                    "the original diagnostics must be preserved: {source}"
                );
            }),
        )
        .await
        .expect("unrelated-table rebuild scenario timed out");
    });
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

                // Finding 7: while the Applied tail still owns the fence, a
                // DIRECT PLAN for the next ordinal must refuse with the same
                // code as STATUS and APPLY — before any retry.
                let plan_error = run_plan(
                    &db,
                    "alpha",
                    predecessor(Some("alpha-001"), 1, digest),
                    vec![submission(&body("alpha-002", 2))],
                    &identity,
                )
                .await
                .expect_err("PLAN must refuse while the Applied tail owns the fence");
                assert_eq!(
                    plan_error.code(),
                    Some("schema_migration_in_doubt"),
                    "PLAN must refuse in doubt: {plan_error}"
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
                // Positive control: the same direct PLAN now succeeds.
                let next = body("alpha-002", 2);
                let planned = run_plan(
                    &db,
                    "alpha",
                    predecessor(Some("alpha-001"), 1, digest),
                    vec![submission(&next)],
                    &identity,
                )
                .await
                .expect("PLAN succeeds after the durable FenceReleased");
                let planned_migration = field(&planned, "migration");
                assert_eq!(str_field(planned_migration, "id"), "alpha-002");
                assert_eq!(field(planned_migration, "ordinal").as_i64(), Some(2));
                assert_eq!(
                    field(&planned, "checksum"),
                    &QueryValue::Bin(checksum(&next).to_vec())
                );
                assert_eq!(
                    field(&planned, "predecessor"),
                    &QueryValue::Map({
                        let mut m = new_map();
                        m.insert("migration_id".into(), QueryValue::Str("alpha-001".into()));
                        m.insert("ordinal".into(), QueryValue::Int(1));
                        m.insert("checksum".into(), QueryValue::Bin(digest.to_vec()));
                        m
                    })
                );
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
