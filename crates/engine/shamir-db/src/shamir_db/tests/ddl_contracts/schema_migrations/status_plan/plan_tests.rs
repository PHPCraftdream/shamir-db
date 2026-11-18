//! Server-side tests for the side-effect-free application-schema PLAN
//! operation (R1c step 2b-2, design §3.3, §4.1, §4.4, §5.2, §5.3):
//! `Manage` authorization, no journal/catalogue mutation, server-recomputed
//! checksums, ordering proven against the durable applied prefix, stable
//! refusal codes, warnings by rule, and the typed wire round trip.
//!
//! Requests are built ONLY through `shamir-query-builder`
//! (`application_schema_plan`) and executed through the normal batch entry
//! point (`execute` / `execute_as` / `to_request_via_msgpack`).

use shamir_query_builder::application_schema_plan;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::SchemaMigration;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationBody, SchemaMigrationOp, SchemaMigrationPlanRequest, SchemaMigrationPredecessor,
    SchemaMigrationSubmission, SchemaMigrationTarget,
};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::helpers::{body, checksum, context, persistent_db};
use crate::shamir_db::system_store::{DatabaseIdentity, JournalEventKind, SchemaMigrationStep};
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// Stage one fully applied migration through the 2a journal API and release
/// its fence, returning (identity, checksum of the applied body).
async fn stage_applied(db: &ShamirDb, name: &str) -> (DatabaseIdentity, [u8; 32]) {
    let store = db.system_store();
    let identity = store.schema_migration_identity(name).await.unwrap();
    let migration = body("alpha-001", 1);
    let metadata = migration.metadata.clone();
    let digest = checksum(&migration);
    let generation = store
        .acquire_schema_migration_fence(identity, 0, metadata.clone(), migration, false, &context())
        .await
        .unwrap();
    for step in [
        SchemaMigrationStep {
            kind: JournalEventKind::OperationIntent,
            operation_index: Some(0),
            evidence: None,
        },
        SchemaMigrationStep {
            kind: JournalEventKind::OperationComplete,
            operation_index: Some(0),
            evidence: Some("table exists with the intended definition".into()),
        },
        SchemaMigrationStep {
            kind: JournalEventKind::Applied,
            operation_index: None,
            evidence: Some("all schema postconditions verified".into()),
        },
    ] {
        store
            .append_schema_migration_event(
                identity,
                generation,
                metadata.clone(),
                digest,
                step,
                &context(),
            )
            .await
            .unwrap();
    }
    store
        .release_schema_migration_fence(identity, generation, metadata, digest, &context())
        .await
        .unwrap();
    (identity, digest)
}

fn submission(body: &SchemaMigrationBody) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(body, checksum(body))
}

fn submission_with_checksum(
    body: &SchemaMigrationBody,
    checksum: [u8; 32],
) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(body, checksum)
}

fn predecessor(id: Option<&str>, ordinal: u64, checksum: [u8; 32]) -> SchemaMigrationPredecessor {
    SchemaMigrationPredecessor {
        migration_id: id.map(str::to_string),
        ordinal,
        checksum,
    }
}

fn plan_request(
    identity: &DatabaseIdentity,
    expected: SchemaMigrationPredecessor,
    migrations: Vec<SchemaMigrationSubmission>,
) -> SchemaMigrationPlanRequest {
    SchemaMigrationPlanRequest {
        protocol_version: 1,
        target: SchemaMigrationTarget {
            database_identity: identity.0,
        },
        expected_predecessor: expected,
        migrations,
    }
}

async fn run_plan(
    db: &ShamirDb,
    request: SchemaMigrationPlanRequest,
) -> Result<QueryValue, crate::query::batch::BatchError> {
    let mut b = Batch::new();
    b.op("plan", application_schema_plan(request));
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute("testdb", &request).await?;
    Ok(response.results["plan"].records[0].as_value().into_owned())
}

fn field<'a>(value: &'a QueryValue, key: &str) -> &'a QueryValue {
    value
        .get(key)
        .unwrap_or_else(|| panic!("field {key} missing"))
}

fn str_field<'a>(value: &'a QueryValue, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("field {key} is not a string"))
}

fn assert_code(error: crate::query::batch::BatchError, expected: &str) {
    assert_eq!(
        error.code(),
        Some(expected),
        "wrong refusal code; message: {error}"
    );
}

#[tokio::test]
async fn plan_requires_manage_and_is_refused_with_read_only() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap();
    let request = plan_request(
        &identity,
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-001", 1))],
    );

    // A non-owner reaching the database through the "other" read bit holds
    // Read but not Manage (Manage is the owner/admin-only class).
    db.set_resource_meta(
        &ResourcePath::database("testdb"),
        &ResourceMeta {
            owner: Actor::User(7),
            group: None,
            mode: 0o004,
        },
    )
    .await
    .unwrap();
    let mut denied_batch = Batch::new();
    denied_batch.op("plan", application_schema_plan(request.clone()));
    let denied_request = denied_batch.to_request_via_msgpack().unwrap();
    let denied = db
        .execute_as(Actor::User(8), "testdb", &denied_request)
        .await
        .expect_err("a Read-only caller must be refused");
    assert_eq!(
        denied.code(),
        Some("access_denied"),
        "expected an access denial, got: {denied:?}"
    );

    // The owner with full mode holds Manage and is admitted.
    db.set_resource_meta(
        &ResourcePath::database("testdb"),
        &ResourceMeta {
            owner: Actor::User(7),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    let mut allowed_batch = Batch::new();
    allowed_batch.op("plan", application_schema_plan(request));
    let allowed_request = allowed_batch.to_request_via_msgpack().unwrap();
    db.execute_as(Actor::User(7), "testdb", &allowed_request)
        .await
        .expect("the owner holds Manage and plan is admitted");
}

#[tokio::test]
async fn plan_returns_the_typed_plan_for_the_next_migration() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let (identity, applied_digest) = stage_applied(&db, "testdb").await;
    let next = body("alpha-002", 2);
    let next_checksum = checksum(&next);
    let request = plan_request(
        &identity,
        predecessor(Some("alpha-001"), 1, applied_digest),
        vec![submission(&next)],
    );

    let value = run_plan(&db, request).await.expect("plan succeeds");
    let migration = field(&value, "migration");
    assert_eq!(str_field(migration, "id"), "alpha-002");
    assert_eq!(field(migration, "ordinal").as_i64(), Some(2));
    assert_eq!(
        field(&value, "checksum"),
        &QueryValue::Bin(next_checksum.to_vec())
    );
    let planned_predecessor = field(&value, "predecessor");
    assert_eq!(str_field(planned_predecessor, "migration_id"), "alpha-001");
    assert_eq!(field(planned_predecessor, "ordinal").as_i64(), Some(1));
    assert_eq!(
        field(planned_predecessor, "checksum"),
        &QueryValue::Bin(applied_digest.to_vec())
    );
    let operations = match field(&value, "operations") {
        QueryValue::List(operations) => operations,
        other => panic!("operations must be a list, got {other:?}"),
    };
    assert_eq!(operations.len(), 1);
    let operation = &operations[0];
    assert_eq!(str_field(operation, "operation_kind"), "create_table");
    assert_eq!(str_field(operation, "target"), "items_2");
    assert_eq!(field(operation, "operation_index").as_i64(), Some(0));
    let warnings = match field(&value, "warnings") {
        QueryValue::List(warnings) => warnings,
        other => panic!("warnings must be a list, got {other:?}"),
    };
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str() == Some("maintenance_fence_required")),
        "apply always requires the maintenance fence: {warnings:?}"
    );
    assert_eq!(
        field(&value, "requires_server_wide_fence"),
        &QueryValue::Bool(false)
    );
    assert!(value.get("recovery_summary").is_some());
}

#[tokio::test]
async fn plan_leaves_the_journal_and_catalogue_untouched() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let (identity, applied_digest) = stage_applied(&db, "testdb").await;
    let journal = db
        .system_store()
        .system_repo()
        .unwrap()
        .get_table("schema_migration_events")
        .await
        .unwrap();
    let databases = db
        .system_store()
        .system_repo()
        .unwrap()
        .get_table("databases")
        .await
        .unwrap();
    let before_journal = super::helpers::rows(&journal).await;
    let before_databases = super::helpers::rows(&databases).await;

    let request = plan_request(
        &identity,
        predecessor(Some("alpha-001"), 1, applied_digest),
        vec![submission(&body("alpha-002", 2))],
    );
    run_plan(&db, request).await.expect("plan succeeds");

    assert_eq!(
        super::helpers::rows(&journal).await,
        before_journal,
        "plan must write no journal event"
    );
    assert_eq!(
        super::helpers::rows(&databases).await,
        before_databases,
        "plan must not mutate the catalogue"
    );
}

#[tokio::test]
async fn plan_refuses_a_checksum_mismatch() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap();
    let request = plan_request(
        &identity,
        predecessor(None, 0, [0; 32]),
        vec![submission_with_checksum(&body("alpha-001", 1), [9; 32])],
    );
    let error = run_plan(&db, request)
        .await
        .expect_err("a wrong client checksum must be refused");
    assert_code(error, "schema_migration_checksum_mismatch");
}

#[tokio::test]
async fn plan_refuses_a_wrong_expected_predecessor() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let (identity, applied_digest) = stage_applied(&db, "testdb").await;
    // The journal head is alpha-001/1; an empty predecessor is stale.
    let request = plan_request(
        &identity,
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-002", 2))],
    );
    let error = run_plan(&db, request)
        .await
        .expect_err("a stale predecessor must be refused");
    assert_code(error, "schema_migration_predecessor_mismatch");

    // A matching ordinal/id with a wrong checksum is refused too.
    let request = plan_request(
        &identity,
        predecessor(Some("alpha-001"), 1, [7; 32]),
        vec![submission(&body("alpha-002", 2))],
    );
    let error = run_plan(&db, request)
        .await
        .expect_err("a wrong predecessor checksum must be refused");
    assert_code(error, "schema_migration_predecessor_mismatch");
    let _ = applied_digest;
}

#[tokio::test]
async fn plan_refuses_gap_duplicate_and_reorder() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let (identity, applied_digest) = stage_applied(&db, "testdb").await;
    let expected = predecessor(Some("alpha-001"), 1, applied_digest);

    // Gap: skips ordinal 2.
    let request = plan_request(
        &identity,
        expected.clone(),
        vec![submission(&body("alpha-003", 3))],
    );
    let error = run_plan(&db, request).await.expect_err("gap refused");
    assert_code(error, "schema_migration_gap");

    // Duplicate: two submissions claim ordinal 2.
    let request = plan_request(
        &identity,
        expected.clone(),
        vec![
            submission(&body("alpha-002", 2)),
            submission(&body("alpha-002-dup", 2)),
        ],
    );
    let error = run_plan(&db, request).await.expect_err("duplicate refused");
    assert_code(error, "schema_migration_gap");

    // Reorder: ordinal 3 arrives before ordinal 2.
    let request = plan_request(
        &identity,
        expected,
        vec![
            submission(&body("alpha-003", 3)),
            submission(&body("alpha-002", 2)),
        ],
    );
    let error = run_plan(&db, request).await.expect_err("reorder refused");
    assert_code(error, "schema_migration_gap");
}

#[tokio::test]
async fn plan_refuses_a_changed_body_for_an_applied_ordinal_without_a_write() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let (identity, applied_digest) = stage_applied(&db, "testdb").await;
    // Same id and ordinal as the applied migration, a different body, and a
    // SELF-CONSISTENT checksum: only the durable prefix can expose the drift.
    let redefined = SchemaMigration::new("alpha-001", 1, "migration 1 redefined")
        .op(ddl::create_table("different_table").build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid");
    let journal = db
        .system_store()
        .system_repo()
        .unwrap()
        .get_table("schema_migration_events")
        .await
        .unwrap();
    let before = super::helpers::rows(&journal).await;
    let request = plan_request(
        &identity,
        predecessor(Some("alpha-001"), 1, applied_digest),
        vec![submission(&redefined)],
    );
    let error = run_plan(&db, request)
        .await
        .expect_err("a redefined applied ordinal must be refused");
    assert_code(error, "schema_migration_checksum_mismatch");
    assert_eq!(
        super::helpers::rows(&journal).await,
        before,
        "the refusal must write nothing"
    );
}

#[tokio::test]
async fn plan_reports_identity_mismatch_for_a_foreign_target() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    db.create_db("other").await.unwrap();
    let foreign = db
        .system_store()
        .schema_migration_identity("other")
        .await
        .unwrap();
    let request = plan_request(
        &foreign,
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-001", 1))],
    );
    let error = run_plan(&db, request)
        .await
        .expect_err("a foreign target must be refused");
    assert_code(error, "schema_migration_identity_mismatch");
}

#[tokio::test]
async fn plan_refuses_while_a_migration_is_in_flight_or_in_doubt() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let store = db.system_store();
    let identity = store.schema_migration_identity("testdb").await.unwrap();
    let migration = body("alpha-001", 1);
    let metadata = migration.metadata.clone();
    let digest = checksum(&migration);
    let generation = store
        .acquire_schema_migration_fence(identity, 0, metadata.clone(), migration, false, &context())
        .await
        .unwrap();

    // Owned fence, no effect attempted yet: in progress.
    let request = plan_request(
        &identity,
        predecessor(None, 0, [0; 32]),
        vec![submission(&body("alpha-002", 2))],
    );
    let error = run_plan(&db, request.clone())
        .await
        .expect_err("an owned fence refuses plan");
    assert_code(error, "schema_migration_in_progress");

    // After a durable operation intent the migration is in doubt.
    store
        .append_schema_migration_event(
            identity,
            generation,
            metadata,
            digest,
            SchemaMigrationStep {
                kind: JournalEventKind::OperationIntent,
                operation_index: Some(0),
                evidence: None,
            },
            &context(),
        )
        .await
        .unwrap();
    let error = run_plan(&db, request)
        .await
        .expect_err("an in-doubt migration refuses plan");
    assert_code(error, "schema_migration_in_doubt");
}

#[test]
fn non_admitted_operations_cannot_enter_a_plan_request() {
    // The admitted subset is enforced by the typed model itself, so a
    // non-admitted operation cannot be put into a request (client side or
    // wire decode) in the first place.
    let not_admitted = shamir_query_types::admin::types::CreateIndexOp {
        create_index: "ix_custom".to_string(),
        table: "items_1".to_string(),
        fields: vec![vec!["name".to_string()]],
        unique: false,
        sorted: false,
        repo: "default".to_string(),
        index_type: Some("functional".to_string()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("custom_fn".to_string()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };
    let error = SchemaMigrationOp::try_from(shamir_query_types::batch::BatchOp::CreateIndex(
        not_admitted,
    ))
    .expect_err("a non-admitted operation must be refused by the typed model");
    assert!(
        error.to_string().contains("functional-index allowlist"),
        "the typed reason must name the offending operation: {error}"
    );
}

#[tokio::test]
async fn plan_lists_function_and_validator_warnings_and_registrations() {
    let (_dir, db) = persistent_db().await;
    db.create_db("testdb").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap();
    let global = SchemaMigration::new("alpha-001", 1, "server-global registrations")
        .op(ddl::create_table("items_1").build())
        .expect("typed create-table operation is admitted")
        .op(ddl::create_function("pricing_fn")
            .source("pub fn shamir_call() -> i64 { 1 }")
            .build()
            .expect("typed create-function operation builds"))
        .expect("typed create-function operation is admitted")
        .op(ddl::create_validator("row_guard")
            .source("pub fn shamir_validate() -> bool { true }")
            .build()
            .expect("typed create-validator operation builds"))
        .expect("typed create-validator operation is admitted")
        .build()
        .expect("test migration body is valid");
    let request = plan_request(
        &identity,
        predecessor(None, 0, [0; 32]),
        vec![submission(&global)],
    );

    let value = run_plan(&db, request).await.expect("plan succeeds");
    let warnings = match field(&value, "warnings") {
        QueryValue::List(warnings) => warnings,
        other => panic!("warnings must be a list, got {other:?}"),
    };
    for expected in [
        "maintenance_fence_required",
        "server_global_registration_blast_radius",
        "dependency_list_must_be_reviewed",
    ] {
        assert!(
            warnings
                .iter()
                .any(|warning| warning.as_str() == Some(expected)),
            "warning {expected} must be present: {warnings:?}"
        );
    }
    assert_eq!(
        field(&value, "requires_server_wide_fence"),
        &QueryValue::Bool(true)
    );
    let operations = match field(&value, "operations") {
        QueryValue::List(operations) => operations,
        other => panic!("operations must be a list, got {other:?}"),
    };
    let mut registered = Vec::new();
    for operation in operations {
        if let Some(QueryValue::List(names)) = operation.get("affected_database_registrations") {
            for name in names {
                registered.push(name.as_str().unwrap_or_default().to_string());
            }
        }
    }
    assert!(
        registered.contains(&"pricing_fn".to_string()),
        "the function registration must be listed: {registered:?}"
    );
    assert!(
        registered.contains(&"row_guard".to_string()),
        "the validator registration must be listed: {registered:?}"
    );
}
