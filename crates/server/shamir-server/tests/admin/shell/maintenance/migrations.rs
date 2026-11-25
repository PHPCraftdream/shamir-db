//! R5 PR5 shell maintenance test: `schema-migration status` reports an
//! APPLIED migration. The migration is applied over the REAL wire with the
//! typed builders (ONE create-table op); the SHELL then re-reads the status
//! view and must project the applied prefix — decimal-string counters,
//! lowercase-hex checksums, no error leaf.
//!
//! Drives the REAL production shell through the shared helpers of
//! [`super::harness`]; every fixture await is a bounded hang detector.

use serde_json::Value;
use zeroize::Zeroizing;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::SchemaMigration;
use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationApplyRequest, SchemaMigrationBody,
    SchemaMigrationPredecessor, SchemaMigrationSubmission, SchemaMigrationTarget,
};
use shamir_types::types::record_id::RecordId;

use super::harness::{
    boot, connect_tls_owned, overwrite_profile, run_shell, seed_catalogue, ShellFixture, DB_NAME,
    REPO_NAME,
};
use super::state_data_common::{data, scram_profile, BOUND};

/// A well-formed Base58 `RecordId` that is not any fixture database's identity.
const FOREIGN_DATABASE_ID: &str = "1111111111111111";
/// The one migration this test applies through the wire.
const MIGRATION_ID: &str = "shell-001";
/// The table the migration creates, in the seeded Fjall repository (an apply
/// names repositories that must already exist on the database).
const MIGRATED_TABLE: &str = "shell_migration_ops";

/// The applied migration body: ONE typed create-table operation.
fn applied_body() -> SchemaMigrationBody {
    SchemaMigration::new(MIGRATION_ID, 1, "shell maintenance applied migration")
        .op(ddl::create_table(MIGRATED_TABLE).repo(REPO_NAME).build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid")
}

/// One `schema-migration status` run projected through the shared `data`
/// helper (exit 0, one LF-terminated result record).
fn status(fixture: &ShellFixture, identity: &str) -> Value {
    fixture.guard.mark("maintenance: schema-migration status");
    let run = run_shell(
        fixture,
        &[
            "--",
            "schema-migration",
            "status",
            "--database-id",
            identity,
        ],
    );
    data(&run, "schema-migration")
}

/// Apply [`applied_body`] as `identity`'s first migration over the REAL wire.
/// A refusal is an envelope-level error, so a result RECORD is required.
fn apply_migration(fixture: &ShellFixture, identity: &str) {
    let body = applied_body();
    let checksum = canonical_body_checksum(&body).expect("the body canonicalizes");
    let request = SchemaMigrationApplyRequest {
        protocol_version: 1,
        target: SchemaMigrationTarget {
            database_identity: identity
                .parse::<RecordId>()
                .expect("shell echoed an identity"),
        },
        expected_predecessor: SchemaMigrationPredecessor {
            migration_id: None,
            ordinal: 0,
            checksum: [0u8; 32],
        },
        expected_fence_generation: 0,
        migration: SchemaMigrationSubmission::new(&body, checksum),
    };
    fixture.guard.mark("maintenance: apply one migration");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(async move {
        let client = tokio::time::timeout(BOUND, connect_tls_owned(addr, pin, password))
            .await
            .expect("apply connect hang bound")
            .expect("apply verified TLS connect");
        let mut batch = Batch::new();
        batch.id("shell_maintenance_apply");
        batch.op(
            "apply",
            shamir_query_builder::apply_application_schema_migration(request),
        );
        let built = batch.try_build().expect("the one-op apply batch validates");
        let response = tokio::time::timeout(BOUND, client.execute(DB_NAME, built))
            .await
            .expect("apply hang bound")
            .expect("the server admits the one-op apply batch");
        let result = response
            .results
            .get("apply")
            .expect("the apply alias is present");
        assert!(!result.skipped, "the apply op must run, not be skipped");
        assert_eq!(
            result.records.len(),
            1,
            "a result record, not an error, is required"
        );
        assert_eq!(
            result.records[0].get_value_str("migration_id"),
            Some(MIGRATION_ID),
            "id"
        );
        assert_eq!(
            result.records[0].get_value_str("status"),
            Some("applied"),
            "status"
        );
        client.close().await;
    });
}

/// One applied migration is visible through the shell's status view: exit 0,
/// `data.error` null, `last_applied_id` / `last_applied_ordinal` /
/// `applied_count` as decimal strings, one `applied` entry whose 64-char
/// lowercase-hex `checksum` equals `last_applied_checksum`, and `in_flight`
/// null or an object.
#[test]
fn schema_migration_status_reports_an_applied_migration() {
    let fixture = boot("schema_migration_status_reports_an_applied_migration");
    seed_catalogue(&fixture);
    let text = scram_profile(&fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(&fixture, &text);

    // The serving identity, discovered through the documented mismatch echo.
    let probe = status(&fixture, FOREIGN_DATABASE_ID);
    assert_eq!(
        probe["error"].as_str(),
        Some("schema_migration_identity_mismatch"),
        "a foreign identity must be a typed mismatch leaf"
    );
    let identity = probe
        .get("database_identity")
        .and_then(Value::as_str)
        .expect("the mismatch echo carries the serving identity")
        .to_owned();
    assert!(
        !identity.is_empty() && identity != FOREIGN_DATABASE_ID,
        "the echo must carry the serving identity, not the probe"
    );

    apply_migration(&fixture, &identity);

    let payload = status(&fixture, &identity);
    assert!(
        payload["error"].is_null(),
        "an applied journal reports no status error"
    );
    assert_eq!(
        payload["last_applied_id"].as_str(),
        Some(MIGRATION_ID),
        "status reports the applied migration id"
    );
    assert_eq!(
        payload["last_applied_ordinal"].as_str(),
        Some("1"),
        "the applied ordinal is the decimal string `1`"
    );
    assert_eq!(
        payload["applied_count"].as_str(),
        Some("1"),
        "the applied-prefix counter is the decimal string `1`"
    );
    let applied = payload["applied"].as_array().expect("`applied` is a list");
    assert_eq!(
        applied.len(),
        1,
        "the applied prefix carries exactly one migration"
    );
    assert_eq!(
        applied[0]["id"].as_str(),
        Some(MIGRATION_ID),
        "the entry names the migration"
    );
    assert_eq!(
        applied[0]["ordinal"].as_str(),
        Some("1"),
        "the entry ordinal is a decimal string"
    );
    let checksum = applied[0]["checksum"]
        .as_str()
        .expect("the entry carries a checksum");
    assert_eq!(checksum.len(), 64, "a checksum renders as 32 bytes of hex");
    assert!(
        checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "a checksum renders as LOWERCASE hex"
    );
    assert_eq!(
        payload["last_applied_checksum"].as_str(),
        Some(checksum),
        "`last_applied_checksum` is the applied entry's checksum"
    );
    let in_flight = &payload["in_flight"];
    assert!(
        in_flight.is_null() || in_flight.is_object(),
        "`in_flight` is null or one in-flight object"
    );
}
