//! Shared fixtures for the resume/reconcile server tests (R1c step 2b-3b).
//! The fixture set mirrors `apply/helpers.rs` (the `apply` module's helpers
//! are `pub(super)` to that module only) and adds the crash/reopen helpers:
//! a durable-journal fault seam arming wrapper and a re-init-over-the-same
//! Fjall directory reopen helper (the `reinit_with_retry` pattern of the
//! system metadata fixtures).

use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationBody, SchemaMigrationPredecessor,
    SchemaMigrationStatusRequest, SchemaMigrationSubmission, SchemaMigrationTarget,
};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::super::helpers::{body, checksum, rows};
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::{JournalEventKind, MigrationHistory, SystemStoreConfig};
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// The database owner: holds `Manage` on the database (and `Create` on the
/// store resource for the per-target DDL preflight).
pub(super) const OWNER: Actor = Actor::User(7);

/// Persistent system store (the journal and the fence need Fjall, never
/// `init_memory`) with `testdb` owned by [`OWNER`] (`0o700`, so the owner
/// holds Manage) and repository `main` granted to the owner.
pub(super) async fn persistent_owner_db() -> (tempfile::TempDir, ShamirDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = ShamirDb::init(SystemStoreConfig::Fjall(dir.path().join("system")))
        .await
        .unwrap();
    db.create_db("testdb").await.unwrap();
    // The migration bodies address repository `main`; create it the way the
    // sibling DDL fixtures do so the store resource `testdb/main` exists
    // before any test (or the admission preflight) addresses it.
    db.add_repo(
        "testdb",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    db.set_resource_meta(
        &ResourcePath::database("testdb"),
        &ResourceMeta {
            owner: OWNER,
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    // §5.2's per-target preflight authorizes CreateTable as `Create` on the
    // STORE resource; grant [`OWNER`] the store too (mirrors the database
    // grant above).
    db.set_resource_meta(
        &ResourcePath::store("testdb", "main"),
        &ResourceMeta {
            owner: OWNER,
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    (dir, db)
}

/// Reopen a NEW `ShamirDb` over the SAME Fjall system directory (the caller
/// must have dropped every previous handle; the TempDir guard stays alive).
/// The bounded retry loop absorbs the backend's asynchronous lock release —
/// the same pattern as the system metadata fixtures' `reinit_with_retry`.
pub(super) async fn reopen_owner_db(dir: &tempfile::TempDir) -> ShamirDb {
    let system_path = dir.path().join("system");
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone())).await {
            Ok(db) => return db,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(system_path))
        .await
        .expect("system store still locked after retries")
}

/// The serving database's canonical identity for `testdb`.
pub(super) async fn identity(db: &ShamirDb) -> crate::shamir_db::system_store::DatabaseIdentity {
    db.system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap()
}

pub(super) fn submission(body: &SchemaMigrationBody) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(body, checksum(body))
}

pub(super) fn predecessor(
    id: Option<&str>,
    ordinal: u64,
    checksum: [u8; 32],
) -> SchemaMigrationPredecessor {
    SchemaMigrationPredecessor {
        migration_id: id.map(str::to_string),
        ordinal,
        checksum,
    }
}

pub(super) fn empty_predecessor() -> SchemaMigrationPredecessor {
    predecessor(None, 0, [0; 32])
}

pub(super) fn apply_request(
    target: &crate::shamir_db::system_store::DatabaseIdentity,
    expected: SchemaMigrationPredecessor,
    fence_generation: u64,
    migration: SchemaMigrationSubmission,
) -> SchemaMigrationApplyRequest {
    SchemaMigrationApplyRequest {
        protocol_version: 1,
        target: SchemaMigrationTarget {
            database_identity: target.0,
        },
        expected_predecessor: expected,
        expected_fence_generation: fence_generation,
        migration,
    }
}

/// Run one apply batch as [`OWNER`] and unwrap the single admin record.
pub(super) async fn run_apply(
    db: &ShamirDb,
    request: SchemaMigrationApplyRequest,
) -> Result<QueryValue, BatchError> {
    let mut b = Batch::new();
    b.op(
        "apply",
        shamir_query_builder::apply_application_schema_migration(request),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute_as(OWNER, "testdb", &request).await?;
    Ok(response.results["apply"].records[0].as_value().into_owned())
}

/// The durable migration journal table (for no-write snapshots).
pub(super) async fn journal(db: &ShamirDb) -> crate::engine::table::TableManager {
    db.system_store()
        .system_repo()
        .unwrap()
        .get_table("schema_migration_events")
        .await
        .unwrap()
}

pub(super) async fn journal_rows(
    db: &ShamirDb,
) -> Vec<(shamir_types::types::record_id::RecordId, QueryValue)> {
    rows(&journal(db).await).await
}

/// The journal event kinds of the migration history, in sequence order.
pub(super) async fn journal_kinds(db: &ShamirDb) -> Vec<JournalEventKind> {
    let target = identity(db).await;
    let history = db
        .system_store()
        .read_schema_migration_history(target)
        .await
        .unwrap();
    history.events.iter().map(|event| event.kind).collect()
}

/// How many durable completions the journal records for one operation index.
pub(super) fn completions_for(history: &MigrationHistory, index: u32) -> usize {
    history
        .events
        .iter()
        .filter(|event| {
            event.kind == JournalEventKind::OperationComplete
                && event.operation_index == Some(index)
        })
        .count()
}

pub(super) fn field<'a>(value: &'a QueryValue, key: &str) -> &'a QueryValue {
    value
        .get(key)
        .unwrap_or_else(|| panic!("field {key} missing"))
}

pub(super) fn str_field<'a>(value: &'a QueryValue, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("field {key} is not a string"))
}

pub(super) fn assert_code(error: BatchError, expected: &str) {
    assert_eq!(
        error.code(),
        Some(expected),
        "wrong refusal code; message: {error}"
    );
}

/// Crash a one-operation create migration (`body("alpha-001", 1)`) at its
/// FIRST operation-intent append (append 2 → tick 3) and reopen the same
/// Fjall directory: the journal holds `started` only and the reopen seeds
/// the in-doubt marker. Shared crash pattern of the chunk-1 tests.
pub(super) async fn crash_started_only_and_reopen(
) -> (tempfile::TempDir, ShamirDb, SchemaMigrationBody) {
    let (dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let migration = body("alpha-001", 1);
    db.system_store().arm_durability_tail_faults_after(2, 1);
    let error = run_apply(
        &db,
        apply_request(&target, empty_predecessor(), 0, submission(&migration)),
    )
    .await
    .expect_err("the failed intent append must stop the run");
    assert_code(error, "schema_migration_durability_unavailable");
    assert_eq!(
        journal_kinds(&db).await,
        vec![JournalEventKind::Started],
        "the journal holds only the started event"
    );
    drop(db);
    let db = reopen_owner_db(&dir).await;
    (dir, db, migration)
}

/// Run the read-only application-schema STATUS op as [`OWNER`] and unwrap
/// the single admin record (builder-only request construction).
pub(super) async fn run_status(
    db: &ShamirDb,
    target: &crate::shamir_db::system_store::DatabaseIdentity,
) -> QueryValue {
    let mut b = Batch::new();
    b.op(
        "status",
        shamir_query_builder::application_schema_status(SchemaMigrationStatusRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: target.0,
            },
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute_as(OWNER, "testdb", &request).await.unwrap();
    response.results["status"].records[0]
        .as_value()
        .into_owned()
}
