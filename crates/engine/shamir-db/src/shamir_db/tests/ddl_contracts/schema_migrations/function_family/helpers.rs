//! Persistent owner fixtures shared by the function-family migration tests.

use std::{future::Future, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use shamir_query_builder::{batch::Batch, ddl, SchemaMigration};
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationBody, SchemaMigrationPlanRequest,
    SchemaMigrationPredecessor, SchemaMigrationSubmission, SchemaMigrationTarget,
};
use shamir_query_types::batch::BatchOp;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::super::helpers::{checksum, persistent_db};
use crate::engine::repo::{repo_types::BoxRepoFactory, RepoConfig};
use crate::query::batch::{BatchError, BatchResponse};
use crate::shamir_db::system_store::{DatabaseIdentity, MigrationHistory};
use crate::types::value::QueryValue;
use crate::ShamirDb;

pub(super) const OWNER: Actor = Actor::User(7);
pub(super) const DB: &str = "testdb";
pub(super) const REPO: &str = "main";

/// Bound each step on the test's current-thread runtime. Boxing keeps the
/// engine futures out of the enclosing test future's stack storage.
pub(super) async fn bounded<T>(step: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), Box::pin(step))
        .await
        .expect("function-family fixture step timed out")
}

pub(super) async fn persistent_owner_db() -> (tempfile::TempDir, ShamirDb) {
    let (dir, db) = bounded(persistent_db()).await;
    bounded(db.create_db(DB)).await.unwrap();
    let repo_path = dir.path().join(REPO);
    bounded(db.add_repo(DB, RepoConfig::new(REPO, BoxRepoFactory::fjall(&repo_path))))
        .await
        .unwrap();
    let meta = ResourceMeta {
        owner: OWNER,
        group: None,
        mode: 0o700,
    };
    for path in [
        ResourcePath::database(DB),
        ResourcePath::store(DB, REPO),
        ResourcePath::FunctionNamespace,
        ResourcePath::WasmCompiler,
    ] {
        bounded(db.set_resource_meta(&path, &meta)).await.unwrap();
    }
    (dir, db)
}

pub(super) fn wasm() -> String {
    // The same minimal uploaded-module ABI as the validator admission fixture.
    STANDARD.encode(
        wat::parse_str(
            r#"(module
        (memory (export "memory") 2)
        (func (export "shamir_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0)
    )"#,
        )
        .unwrap(),
    )
}

pub(super) fn create_function(name: &str) -> BatchOp {
    ddl::create_function(name).wasm(wasm()).build().unwrap()
}

pub(super) fn create_validator(name: &str) -> BatchOp {
    ddl::create_validator(name).wasm(wasm()).build().unwrap()
}

pub(super) fn body(
    id: &str,
    ordinal: u64,
    ops: impl IntoIterator<Item = BatchOp>,
) -> SchemaMigrationBody {
    let mut builder = SchemaMigration::new(id, ordinal, id);
    for op in ops {
        builder = builder.op(op).unwrap();
    }
    builder.build().unwrap()
}

pub(super) async fn identity(db: &ShamirDb) -> DatabaseIdentity {
    bounded(db.system_store().schema_migration_identity(DB))
        .await
        .unwrap()
}

pub(super) async fn history(db: &ShamirDb, target: DatabaseIdentity) -> MigrationHistory {
    bounded(db.system_store().read_schema_migration_history(target))
        .await
        .unwrap()
}

pub(super) fn apply_request(
    target: DatabaseIdentity,
    prior: Option<&SchemaMigrationBody>,
    generation: u64,
    body: &SchemaMigrationBody,
) -> SchemaMigrationApplyRequest {
    SchemaMigrationApplyRequest {
        protocol_version: 1,
        target: SchemaMigrationTarget {
            database_identity: target.0,
        },
        expected_predecessor: predecessor(prior),
        expected_fence_generation: generation,
        migration: SchemaMigrationSubmission::new(body, checksum(body)),
    }
}

fn predecessor(prior: Option<&SchemaMigrationBody>) -> SchemaMigrationPredecessor {
    SchemaMigrationPredecessor {
        migration_id: prior.map(|body| body.metadata.id.clone()),
        ordinal: prior.map_or(0, |body| body.metadata.ordinal),
        checksum: prior.map_or([0; 32], checksum),
    }
}

pub(super) async fn run_batch(db: &ShamirDb, batch: Batch) -> Result<BatchResponse, BatchError> {
    let request = batch.to_request_via_msgpack().unwrap();
    bounded(db.execute_as(OWNER, DB, &request)).await
}

pub(super) async fn run_op(db: &ShamirDb, op: BatchOp) -> Result<QueryValue, BatchError> {
    let mut batch = Batch::new();
    batch.op("op", op);
    let response = run_batch(db, batch).await?;
    Ok(response.results["op"].records[0].as_value().into_owned())
}

pub(super) async fn run_apply(
    db: &ShamirDb,
    request: SchemaMigrationApplyRequest,
) -> Result<QueryValue, BatchError> {
    run_op(
        db,
        shamir_query_builder::apply_application_schema_migration(request),
    )
    .await
}

pub(super) async fn run_plan(
    db: &ShamirDb,
    target: DatabaseIdentity,
    body: &SchemaMigrationBody,
) -> Result<QueryValue, BatchError> {
    run_op(
        db,
        shamir_query_builder::application_schema_plan(SchemaMigrationPlanRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: target.0,
            },
            expected_predecessor: predecessor(None),
            migrations: vec![SchemaMigrationSubmission::new(body, checksum(body))],
        }),
    )
    .await
}
