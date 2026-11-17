//! Server-side tests for the read-only application-schema status operation
//! (R1c step 2b-1): empty journal, applied prefix, owned fence, in-doubt,
//! checksum mismatch, identity mismatch, authorization, and the documented
//! temporary plan/apply refusal.
//!
//! Requests are built ONLY through `shamir-query-builder`
//! (`application_schema_status`) and executed through the normal batch
//! entry point (`execute_as` / `to_request_via_msgpack`).

use shamir_query_builder::application_schema_plan;
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationPlanRequest, SchemaMigrationStatusRequest,
    SchemaMigrationTarget,
};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use super::helpers::{body, checksum, context, rows};
use crate::shamir_db::system_store::{DatabaseIdentity, JournalEventKind, SchemaMigrationStep};
use crate::types::value::QueryValue;
use crate::ShamirDb;

/// Stage a fully applied migration through the 2a journal API and release
/// its fence, returning (identity, checksum).
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

fn status_op(identity: DatabaseIdentity) -> shamir_query_types::batch::BatchOp {
    shamir_query_builder::application_schema_status(SchemaMigrationStatusRequest {
        protocol_version: 1,
        target: SchemaMigrationTarget {
            database_identity: identity.0,
        },
    })
}

async fn run_status(db: &ShamirDb, identity: DatabaseIdentity) -> QueryValue {
    let mut b = Batch::new();
    b.op("status", status_op(identity));
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute("testdb", &request).await.unwrap();
    response.results["status"].records[0]
        .as_value()
        .into_owned()
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

#[tokio::test]
async fn status_of_an_empty_journal_reports_not_checked() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap();

    let value = run_status(&db, identity).await;
    assert_eq!(field(&value, "last_applied_ordinal").as_i64(), Some(0));
    assert!(value.get("last_applied_id").is_none());
    assert_eq!(str_field(&value, "checksum_validation"), "not_checked");
    assert!(value.get("in_flight").is_none());
    assert!(value.get("error").is_none());
}

#[tokio::test]
async fn status_after_applied_events_reports_last_applied_and_valid_checksum() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let (identity, digest) = stage_applied(&db, "testdb").await;

    let value = run_status(&db, identity).await;
    assert_eq!(str_field(&value, "last_applied_id"), "alpha-001");
    assert_eq!(field(&value, "last_applied_ordinal").as_i64(), Some(1));
    assert_eq!(
        field(&value, "last_applied_checksum"),
        &QueryValue::Bin(digest.to_vec())
    );
    assert_eq!(str_field(&value, "checksum_validation"), "valid");
    assert!(value.get("in_flight").is_none());
    assert!(value.get("error").is_none());
}

#[tokio::test]
async fn status_with_an_owned_fence_reports_the_in_flight_owner() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let store = db.system_store();
    let identity = store.schema_migration_identity("testdb").await.unwrap();
    let migration = body("alpha-001", 1);
    let metadata = migration.metadata.clone();
    store
        .acquire_schema_migration_fence(identity, 0, metadata, migration, false, &context())
        .await
        .unwrap();

    let value = run_status(&db, identity).await;
    let in_flight = field(&value, "in_flight");
    assert_eq!(str_field(in_flight, "migration_id"), "alpha-001");
    assert_eq!(field(in_flight, "ordinal").as_i64(), Some(1));
    assert_eq!(field(in_flight, "fence_generation").as_i64(), Some(1));
    assert_eq!(str_field(in_flight, "status"), "started");
    assert_eq!(str_field(&value, "error"), "schema_migration_in_doubt");
}

#[tokio::test]
async fn status_in_doubt_reports_the_pending_operation() {
    let db = ShamirDb::init_memory().await.unwrap();
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

    let value = run_status(&db, identity).await;
    let in_flight = field(&value, "in_flight");
    assert_eq!(field(in_flight, "operation_index").as_i64(), Some(0));
    assert_eq!(str_field(in_flight, "status"), "operation_intent");
    assert_eq!(str_field(&value, "error"), "schema_migration_in_doubt");
}

#[tokio::test]
async fn status_detects_a_checksum_mismatch_between_body_and_applied_record() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let (identity, _digest) = stage_applied(&db, "testdb").await;

    // Corrupt ONLY the stored body of the started event: the recorded
    // checksums stay consistent, so the history validates but the
    // server-recomputed canonical checksum no longer matches.
    let table = db
        .system_store()
        .system_repo()
        .unwrap()
        .get_table("schema_migration_events")
        .await
        .unwrap();
    let (event_id, mut envelope) = rows(&table)
        .await
        .into_iter()
        .find(|(_, row)| row.get("record_type").and_then(QueryValue::as_str) == Some("event"))
        .unwrap();
    let payload = match envelope.get("event") {
        Some(QueryValue::Bin(bytes)) => bytes.clone(),
        _ => panic!("started event payload is binary"),
    };
    let mut event: crate::shamir_db::system_store::JournalEvent =
        rmp_serde::from_slice(&payload).unwrap();
    if let Some(body) = event.body.as_mut() {
        let extra = shamir_query_builder::ddl::create_table("unplanned_items").build();
        body.operations.push(
            shamir_query_types::admin::types::schema_migration::SchemaMigrationOp::try_from(extra)
                .expect("typed create-table operation is admitted"),
        );
    }
    if let QueryValue::Map(fields) = &mut envelope {
        fields.insert(
            "event".into(),
            QueryValue::Bin(rmp_serde::to_vec_named(&event).unwrap()),
        );
    }
    super::helpers::overwrite_row(&table, event_id, envelope).await;

    let value = run_status(&db, identity).await;
    assert_eq!(str_field(&value, "checksum_validation"), "mismatch");
}

#[tokio::test]
async fn status_reports_identity_mismatch_for_a_foreign_target() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    db.create_db("other").await.unwrap();
    let foreign = db
        .system_store()
        .schema_migration_identity("other")
        .await
        .unwrap();

    let value = run_status(&db, foreign).await;
    assert_eq!(
        str_field(&value, "error"),
        "schema_migration_identity_mismatch"
    );
    assert_eq!(field(&value, "last_applied_ordinal").as_i64(), Some(0));
    assert_eq!(str_field(&value, "checksum_validation"), "not_checked");
}

#[tokio::test]
async fn status_requires_database_read() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.op("status", status_op(identity));
    let request = b.to_request_via_msgpack().unwrap();

    // A user with no grant is refused.
    let denied = db
        .execute_as(Actor::User(99), "testdb", &request)
        .await
        .expect_err("a caller without access must be refused");
    let denied_message = format!("{denied:?}");
    assert!(
        denied_message.contains("denied") || denied_message.contains("Denied"),
        "expected an access denial, got: {denied_message}"
    );

    // The owner holds Read and is admitted.
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
    let allowed = db
        .execute_as(Actor::User(7), "testdb", &request)
        .await
        .expect("the database owner holds Read");
    let value = allowed.results["status"].records[0].as_value().into_owned();
    assert_eq!(field(&value, "last_applied_ordinal").as_i64(), Some(0));
}

#[tokio::test]
async fn plan_and_apply_arms_return_the_documented_temporary_error() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let identity = db
        .system_store()
        .schema_migration_identity("testdb")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.op(
        "plan",
        application_schema_plan(SchemaMigrationPlanRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
            expected_predecessor:
                shamir_query_types::admin::types::schema_migration::SchemaMigrationPredecessor {
                    migration_id: None,
                    ordinal: 0,
                    checksum: [0; 32],
                },
            migrations: Vec::new(),
        }),
    );
    b.op(
        "apply",
        shamir_query_builder::apply_application_schema_migration(SchemaMigrationApplyRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
            expected_predecessor:
                shamir_query_types::admin::types::schema_migration::SchemaMigrationPredecessor {
                    migration_id: None,
                    ordinal: 0,
                    checksum: [0; 32],
                },
            expected_fence_generation: 0,
            migration:
                shamir_query_types::admin::types::schema_migration::SchemaMigrationSubmission {
                    metadata: body("alpha-001", 1).metadata,
                    function_dependencies: Vec::new(),
                    operations: Vec::new(),
                    checksum: [0; 32],
                },
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let error = db
        .execute("testdb", &request)
        .await
        .expect_err("plan/apply refuse");
    let message = error.to_string();
    assert!(
        message.contains("schema_migration_operation_not_implemented"),
        "temporary error must be named, got: {message}"
    );
    assert!(
        !message.contains("migration_state_changed") && !message.contains("read_only_replica"),
        "must stay distinguishable from online storage-move / replica errors: {message}"
    );
}
