use crate::access::ResourceMeta;
use crate::shamir_db::system_store::{
    DatabaseIdentity, JournalEventKind, SchemaMigrationStep, SystemStoreConfig,
};
use crate::shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationStatusRequest, SchemaMigrationTarget,
};
use shamir_types::types::value::QueryValue;
use std::str::FromStr;
use std::time::Duration;
use tokio::runtime::Builder;

use super::helpers::{body, checksum, context};

/// Every phase of the touched scenario is bounded well above the 120 s
/// floor (the Windows CI runner can be up to 10x slower) and `Box::pin`s
/// the engine futures so their frames never live in the test thread's
/// stack (debug-test stacks are small).
async fn bounded<T>(step: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(180), Box::pin(step))
        .await
        .expect("identity-restart scenario phase timed out")
}

/// Reopen the persistent system store. A just-dropped engine can keep its
/// fjall lock until its background handles finish (`FjallError: Locked`,
/// seen on Windows), so retry that one error for a bounded time; any other
/// error fails at once. Never a bare reopen.
async fn init_waiting_for_locks(system_path: std::path::PathBuf) -> ShamirDb {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        match bounded(ShamirDb::init(SystemStoreConfig::Fjall(
            system_path.clone(),
        )))
        .await
        {
            Ok(db) => return db,
            Err(error)
                if error.to_string().contains("Locked")
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("reopen the system store: {error}"),
        }
    }
}

/// A typed Batch status request built ONLY through `shamir-query-builder`,
/// executed through the normal batch entry point (`execute`) against the
/// database under `database_name`.
async fn run_status_batch(
    db: &ShamirDb,
    database_name: &str,
    identity: DatabaseIdentity,
) -> QueryValue {
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
    let response = bounded(db.execute(database_name, &request)).await.unwrap();
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

/// The typed status handler reports the SAME stored identity's applied
/// prefix (`alpha-001`, ordinal 1, the staged checksum) with the checksum
/// validated, no in-flight fence and no error — both post-rename and
/// post-restart.
fn assert_applied_prefix_via_status(value: &QueryValue, digest: [u8; 32]) {
    assert_eq!(str_field(value, "last_applied_id"), "alpha-001");
    assert_eq!(field(value, "last_applied_ordinal").as_i64(), Some(1));
    assert_eq!(
        field(value, "last_applied_checksum"),
        &QueryValue::Bin(digest.to_vec())
    );
    assert_eq!(str_field(value, "checksum_validation"), "valid");
    assert!(value.get("in_flight").is_none());
    assert!(value.get("error").is_none());
    let QueryValue::List(entries) = field(value, "applied") else {
        panic!("applied is not a list");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(str_field(&entries[0], "id"), "alpha-001");
    assert_eq!(field(&entries[0], "ordinal").as_i64(), Some(1));
    assert_eq!(
        field(&entries[0], "checksum"),
        &QueryValue::Bin(digest.to_vec())
    );
}

#[test]
fn database_identity_survives_cold_restart_and_rename() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let (identity, digest) = runtime.block_on(bounded(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        db.create_db("before_rename").await.unwrap();
        let identity = db
            .system_store()
            .schema_migration_identity("before_rename")
            .await
            .unwrap();
        // Populate the journal under the pre-rename identity: a fully
        // applied migration (the rename must not disturb the per-identity
        // partition or its applied prefix).
        let store = db.system_store();
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
        db.rename_db("before_rename", "after_rename").await.unwrap();
        assert_eq!(
            db.system_store()
                .schema_migration_identity("after_rename")
                .await
                .unwrap(),
            identity
        );
        // The populated journal is still served under the SAME identity
        // after the rename: applied prefix intact.
        let renamed_history = db
            .system_store()
            .read_schema_migration_history(identity)
            .await
            .unwrap();
        assert_eq!(renamed_history.applied.len(), 1);
        assert_eq!(renamed_history.applied[0].checksum, digest);
        // NEW (4b): the typed Batch status operation, executed through the
        // handler under the NEW database name, reports the SAME stored
        // identity's applied prefix with checksum validity.
        let status = run_status_batch(&db, "after_rename", identity).await;
        assert_applied_prefix_via_status(&status, digest);
        (identity, digest)
    }));
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(bounded(async {
        let db = init_waiting_for_locks(system_path).await;
        assert_eq!(
            db.system_store()
                .schema_migration_identity("after_rename")
                .await
                .unwrap(),
            identity
        );
        // Cold restart after the rename: the journal (including the applied
        // prefix and its canonical body) is still partitioned under the
        // same identity.
        let history = db
            .system_store()
            .read_schema_migration_history(identity)
            .await
            .unwrap();
        assert_eq!(history.applied.len(), 1);
        assert_eq!(
            history.applied[0].migration.id, "alpha-001",
            "the applied prefix survives the rename and the restart"
        );
        // NEW (4b): the same typed status, again through the handler under
        // the NEW name after the cold restart: same stored identity, same
        // applied prefix, checksum still validated.
        let status = run_status_batch(&db, "after_rename", identity).await;
        assert_applied_prefix_via_status(&status, digest);
    }));
    drop(runtime);
}

#[test]
fn dropped_and_recreated_database_gets_a_fresh_identity_and_partition() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let old_identity = runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        db.create_db("recycled").await.unwrap();
        let identity = db
            .system_store()
            .schema_migration_identity("recycled")
            .await
            .unwrap();
        assert!(db.remove_db("recycled").await.unwrap());
        db.create_db("recycled").await.unwrap();
        let recreated = db
            .system_store()
            .schema_migration_identity("recycled")
            .await
            .unwrap();
        assert_ne!(identity, recreated);
        (identity, recreated)
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .unwrap();
        let current = db
            .system_store()
            .schema_migration_identity("recycled")
            .await
            .unwrap();
        assert_eq!(current, old_identity.1);
        let history = db
            .system_store()
            .read_schema_migration_history(current)
            .await
            .unwrap();
        assert!(history.events.is_empty());
        assert_eq!(
            old_identity
                .0
                .to_string()
                .parse::<DatabaseIdentity>()
                .unwrap(),
            old_identity.0
        );
    });
    drop(runtime);
}

#[test]
fn legacy_database_identity_backfill_is_durable_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        db.create_db("legacy").await.unwrap();
        let mut legacy_fields = shamir_types::types::common::new_map();
        legacy_fields.insert("name".into(), QueryValue::Str("legacy".into()));
        legacy_fields.insert("created_at".into(), QueryValue::Int(1));
        // Replace a normal database row with the exact pre-R1c shape. Boot on
        // the next runtime exercises the production catalogue backfill path.
        db.system_store()
            .save_database(
                "legacy",
                &QueryValue::Map(legacy_fields),
                &ResourceMeta::open(),
            )
            .await
            .unwrap();
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let first = runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
            .await
            .unwrap();
        db.system_store()
            .schema_migration_identity("legacy")
            .await
            .unwrap()
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .unwrap();
        assert_eq!(
            db.system_store()
                .schema_migration_identity("legacy")
                .await
                .unwrap(),
            first
        );
        let row = db
            .system_store()
            .load_database("legacy")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.get("identity").and_then(QueryValue::as_str),
            Some(first.to_string().as_str())
        );
    });
    drop(runtime);
}

#[test]
fn database_identity_display_parse_round_trip_rejects_invalid_values() {
    let identity = DatabaseIdentity::new();
    let encoded = identity.to_string();
    assert_eq!(DatabaseIdentity::from_str(&encoded).unwrap(), identity);
    assert!(DatabaseIdentity::from_str("not-a-record-id").is_err());
}
