use crate::access::ResourceMeta;
use crate::shamir_db::system_store::{
    DatabaseIdentity, JournalEventKind, SchemaMigrationStep, SystemStoreConfig,
};
use crate::shamir_db::ShamirDb;
use shamir_types::types::value::QueryValue;
use std::str::FromStr;
use tokio::runtime::Builder;

use super::helpers::{body, checksum, context};

#[test]
fn database_identity_survives_cold_restart_and_rename() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("system");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let identity = runtime.block_on(async {
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
        identity
    });
    drop(runtime);

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .unwrap();
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
    });
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
