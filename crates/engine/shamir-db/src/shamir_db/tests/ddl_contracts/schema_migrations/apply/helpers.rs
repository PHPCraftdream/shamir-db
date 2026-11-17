//! Shared fixtures for the application-schema APPLY server tests
//! (R1c step 2b-3a). Manifest-only sibling modules re-export nothing here;
//! every apply request is built through `shamir-query-builder` and executed
//! through the normal batch entry point (`execute_as`).

use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationBody, SchemaMigrationPredecessor,
    SchemaMigrationSubmission, SchemaMigrationTarget,
};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::super::helpers::{checksum, rows};
use crate::engine::table::TableManager;
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::{DatabaseIdentity, SystemStoreConfig};
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// The database owner: holds `Manage` on the database. The "allowed" half of
/// every authorization test runs through this actor, never the system actor.
pub(super) const OWNER: Actor = Actor::User(7);

/// A non-owner that reaches the database only through the "other" read bit:
/// holds `Read` but not `Manage`.
pub(super) const OTHER: Actor = Actor::User(8);

/// Persistent system store (the journal and the fence need Fjall, never
/// `init_memory`) with `testdb` owned by [`OWNER`] (`0o700`, so the owner
/// holds Manage).
pub(super) async fn persistent_owner_db() -> (tempfile::TempDir, ShamirDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = ShamirDb::init(SystemStoreConfig::Fjall(dir.path().join("system")))
        .await
        .unwrap();
    db.create_db("testdb").await.unwrap();
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
    (dir, db)
}

/// The serving database's canonical identity for `testdb`.
pub(super) async fn identity(db: &ShamirDb) -> DatabaseIdentity {
    db.system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap()
}

/// A `Read`-only (other-read bit) caller is still refused: set the database
/// owner to [`OWNER`] with no "other" bits.
pub(super) async fn grant_other_read_only(db: &ShamirDb) {
    db.set_resource_meta(
        &ResourcePath::database("testdb"),
        &ResourceMeta {
            owner: OWNER,
            group: None,
            mode: 0o004,
        },
    )
    .await
    .unwrap();
}

pub(super) fn submission(body: &SchemaMigrationBody) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(body, checksum(body))
}

/// A submission whose checksum deliberately disagrees with its body
/// (self-inconsistent, like the plan-arm tests).
pub(super) fn submission_with_checksum(
    body: &SchemaMigrationBody,
    checksum: [u8; 32],
) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(body, checksum)
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
    target: &DatabaseIdentity,
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
    run_apply_as(db, OWNER, request).await
}

pub(super) async fn run_apply_as(
    db: &ShamirDb,
    actor: Actor,
    request: SchemaMigrationApplyRequest,
) -> Result<QueryValue, BatchError> {
    let mut b = Batch::new();
    b.op(
        "apply",
        shamir_query_builder::apply_application_schema_migration(request),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute_as(actor, "testdb", &request).await?;
    Ok(response.results["apply"].records[0].as_value().into_owned())
}

/// The durable migration journal table (for no-write snapshots).
pub(super) async fn journal(db: &ShamirDb) -> TableManager {
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
