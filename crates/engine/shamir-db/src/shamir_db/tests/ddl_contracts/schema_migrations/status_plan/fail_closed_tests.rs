//! Unknown journal record version fails CLOSED (design §7.1
//! `schema_migration_status_and_plan_unknown_journal_version_fail_closed`):
//! one journal row stamped with an unknown mandatory `record_version` makes
//! STATUS, PLAN and APPLY of the affected database fail with the stable
//! `schema_migration_durability_unavailable` code naming the corruption —
//! never a guess, a partial read or a silent skip. The detection is
//! read-time: none of the three handlers writes to the journal while
//! refusing.

use shamir_query_builder::application_schema_plan;
use shamir_query_builder::apply_application_schema_migration;
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationPlanRequest, SchemaMigrationPredecessor,
    SchemaMigrationStatusRequest, SchemaMigrationSubmission, SchemaMigrationTarget,
};

use super::helpers::{body, checksum, context, overwrite_row, rows};
use crate::shamir_db::system_store::{JournalEventKind, SchemaMigrationStep, SystemStoreConfig};
use crate::types::value::QueryValue;
use crate::ShamirDb;

async fn run_status(db: &ShamirDb, identity: crate::shamir_db::system_store::DatabaseIdentity) {
    let mut b = Batch::new();
    b.op(
        "status",
        shamir_query_builder::application_schema_status(SchemaMigrationStatusRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let error = db
        .execute("testdb", &request)
        .await
        .expect_err("an unknown mandatory record version must fail status closed");
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("unknown mandatory record version"),
        "{error}"
    );
}

async fn run_plan(
    db: &ShamirDb,
    identity: crate::shamir_db::system_store::DatabaseIdentity,
    migration: &shamir_query_types::admin::types::schema_migration::SchemaMigrationBody,
) {
    let mut b = Batch::new();
    b.op(
        "plan",
        application_schema_plan(SchemaMigrationPlanRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
            expected_predecessor: SchemaMigrationPredecessor {
                migration_id: None,
                ordinal: 0,
                checksum: [0; 32],
            },
            migrations: vec![SchemaMigrationSubmission::new(
                migration,
                checksum(migration),
            )],
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let error = db
        .execute("testdb", &request)
        .await
        .expect_err("an unknown mandatory record version must fail plan closed");
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("unknown mandatory record version"),
        "{error}"
    );
}

async fn run_apply(
    db: &ShamirDb,
    identity: crate::shamir_db::system_store::DatabaseIdentity,
    migration: &shamir_query_types::admin::types::schema_migration::SchemaMigrationBody,
) {
    let mut b = Batch::new();
    b.op(
        "apply",
        apply_application_schema_migration(SchemaMigrationApplyRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
            expected_predecessor: SchemaMigrationPredecessor {
                migration_id: None,
                ordinal: 0,
                checksum: [0; 32],
            },
            expected_fence_generation: 1,
            migration: SchemaMigrationSubmission::new(migration, checksum(migration)),
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let error = db
        .execute("testdb", &request)
        .await
        .expect_err("an unknown mandatory record version must fail apply closed");
    assert_eq!(
        error.code(),
        Some("schema_migration_durability_unavailable"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("unknown mandatory record version"),
        "{error}"
    );
}

#[tokio::test]
async fn unknown_journal_record_version_fails_status_plan_and_apply_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = ShamirDb::init(SystemStoreConfig::Fjall(dir.path().join("system")))
        .await
        .unwrap();
    db.create_db("testdb").await.unwrap();
    let store = db.system_store();
    let identity = store.schema_migration_identity("testdb").await.unwrap();
    let migration = body("alpha-001", 1);
    let metadata = migration.metadata.clone();
    let digest = checksum(&migration);
    let generation = store
        .acquire_schema_migration_fence(
            identity,
            0,
            metadata.clone(),
            migration.clone(),
            false,
            &context(),
        )
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

    // Stamp the durable event envelope with an unknown mandatory record
    // version (the step-2a envelope field, not the inner event bytes) —
    // exactly what a future server writing a newer journal would leave
    // behind for this server.
    let table = store
        .system_repo()
        .unwrap()
        .get_table("schema_migration_events")
        .await
        .unwrap();
    let all_rows = rows(&table).await;
    let (event_id, mut envelope) = all_rows
        .iter()
        .find(|(_, row)| row.get("record_type").and_then(QueryValue::as_str) == Some("event"))
        .expect("the journal holds the event row")
        .clone();
    if let QueryValue::Map(fields) = &mut envelope {
        fields.insert("record_version".into(), QueryValue::Str("99".into()));
    }
    overwrite_row(&table, event_id, envelope).await;

    // STATUS, PLAN and APPLY all fail closed with the same stable code and
    // the exact corruption detail — and none of them writes to the journal.
    run_status(&db, identity).await;
    run_plan(&db, identity, &migration).await;
    run_apply(&db, identity, &migration).await;

    let after = rows(&table).await;
    assert_eq!(
        after.len(),
        all_rows.len(),
        "the refusals are read-time detections: the journal is untouched"
    );
}
