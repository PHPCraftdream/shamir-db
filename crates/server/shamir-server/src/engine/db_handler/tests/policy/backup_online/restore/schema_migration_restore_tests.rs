//! Restored-backup journal rewind (design §7.1
//! `schema_migration_restored_backup_rewinds_journal_and_reapplies`, §2.3):
//! the restored journal's applied prefix is AUTHORITATIVE and rewind is
//! real — not merely a retry of an already applied ordinal.
//!
//! Sequence, over the real engine fixture and the REAL production
//! `restore_with_probe` seam (the supported backup-restore path):
//! 1. apply migration ordinal 1 (create `mig_items`) on the source; capture
//!    the online snapshot AT THAT STATE;
//! 2. apply a SECOND ordinal (create `mig_items_2`) AFTER the capture and
//!    prove it durably applied on the source;
//! 3. restore the EARLIER snapshot to a fresh target: the restored boot's
//!    journal is rewound to the captured prefix (ordinal 1 only, the
//!    ordinal-2 applied entry and its effect absent, fence generation back
//!    at the captured value) and the stable database identity survives;
//! 4. re-apply the IDENTICAL ordinal-2 body verbatim against the restored
//!    engine, with the status-derived expected fence generation: it
//!    succeeds — the rewound journal accepts the forward re-apply, proving
//!    restore-before-upgrade (an already-applied-before-restore ordinal is
//!    not silently treated as applied).
//!
//! Builder-only wire construction throughout; every await is bounded.

use std::future::Future;
use std::time::Duration;

use shamir_db::shamir_db::system_store::DatabaseIdentity;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::application_schema_status;
use shamir_query_builder::apply_application_schema_migration;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::SchemaMigration;
use shamir_query_types::admin::types::schema_migration::canonical_body_checksum;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationBody, SchemaMigrationPredecessor,
    SchemaMigrationStatusRequest, SchemaMigrationSubmission, SchemaMigrationTarget,
};

use super::fixture::{on_big_stack, Fixture};
use crate::sync::durability::backup::restore::{restore_with_probe, RestoreReport};

const SYSTEM_DIR: &str = "shamir_db_meta.redb";
const DB_NAME: &str = "app";

/// Bounded await at the brief's 120 s budget (the Windows runner is up to
/// 10x slower); every facade await in this test goes through it.
async fn bounded<F: Future>(step: &str, future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(120), Box::pin(future))
        .await
        .unwrap_or_else(|_| panic!("{step} timed out"))
}

/// One typed create-table migration body with the given ordinal.
fn migration_body(ordinal: u64, table: &str) -> SchemaMigrationBody {
    SchemaMigration::new(
        format!("restore-rewind-{ordinal}"),
        ordinal,
        format!("migration {ordinal}"),
    )
    .op(ddl::create_table(table).build())
    .expect("typed create-table operation is admitted")
    .build()
    .expect("test migration body is valid")
}

fn submission(body: &SchemaMigrationBody) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(body, canonical_body_checksum(body).unwrap())
}

fn status_batch(identity: &DatabaseIdentity) -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.op(
        "status",
        application_schema_status(SchemaMigrationStatusRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
        }),
    );
    b.build()
}

async fn run_status(
    db: &ShamirDb,
    identity: &DatabaseIdentity,
) -> shamir_types::types::value::QueryValue {
    let request = status_batch(identity);
    let response = db
        .execute(DB_NAME, &request)
        .await
        .expect("status executes");
    response.results["status"].records[0]
        .as_value()
        .into_owned()
}

async fn apply_migration(
    db: &ShamirDb,
    identity: &DatabaseIdentity,
    expected_generation: u64,
    body: &SchemaMigrationBody,
) -> shamir_types::types::value::QueryValue {
    let mut b = Batch::new();
    b.op(
        "apply",
        apply_application_schema_migration(SchemaMigrationApplyRequest {
            protocol_version: 1,
            target: SchemaMigrationTarget {
                database_identity: identity.0,
            },
            expected_predecessor: predecessor_of(body.metadata.ordinal, db, identity).await,
            expected_fence_generation: expected_generation,
            migration: submission(body),
        }),
    );
    let request = b.build();
    let response = db.execute(DB_NAME, &request).await.expect("apply executes");
    response.results["apply"].records[0].as_value().into_owned()
}

/// The predecessor the client derives from status: the last applied entry,
/// or the empty predecessor for ordinal 1.
async fn predecessor_of(
    ordinal: u64,
    db: &ShamirDb,
    identity: &DatabaseIdentity,
) -> SchemaMigrationPredecessor {
    let value = run_status(db, identity).await;
    if ordinal == 1 {
        return SchemaMigrationPredecessor {
            migration_id: None,
            ordinal: 0,
            checksum: [0; 32],
        };
    }
    let previous = ordinal - 1;
    let applied = value
        .get("applied")
        .and_then(|list| match list {
            shamir_types::types::value::QueryValue::List(entries) => Some(entries),
            _ => None,
        })
        .expect("status lists the applied prefix");
    let entry = applied
        .iter()
        .find(|entry| {
            entry
                .get("ordinal")
                .and_then(shamir_types::types::value::QueryValue::as_i64)
                == Some(previous as i64)
        })
        .expect("the previous ordinal is applied");
    let id = entry
        .get("id")
        .and_then(shamir_types::types::value::QueryValue::as_str)
        .expect("applied entry carries its id")
        .to_string();
    let checksum = match entry
        .get("checksum")
        .expect("applied entry carries its checksum")
    {
        shamir_types::types::value::QueryValue::Bin(bytes) => bytes,
        other => panic!("checksum is binary, got: {other:?}"),
    };
    let mut digest = [0u8; 32];
    digest.copy_from_slice(checksum);
    SchemaMigrationPredecessor {
        migration_id: Some(id),
        ordinal: previous,
        checksum: digest,
    }
}

fn field<'a>(
    value: &'a shamir_types::types::value::QueryValue,
    key: &str,
) -> &'a shamir_types::types::value::QueryValue {
    value
        .get(key)
        .unwrap_or_else(|| panic!("field {key} missing"))
}

fn int_field(value: &shamir_types::types::value::QueryValue, key: &str) -> i64 {
    field(value, key)
        .as_i64()
        .unwrap_or_else(|| panic!("field {key} is not an integer"))
}

/// The synchronous production restore on the current (big-stack) thread —
/// the same online-probe contract as the R2a restore tests.
fn run_restore(fixture: &Fixture) -> RestoreReport {
    let from = fixture.dest_root.join("online");
    let target = fixture.target_data_dir();
    restore_with_probe(&from, &target, false, |_path| {
        panic!("legacy liveness probe must not run for an online snapshot");
    })
    .expect("online restore must succeed")
}

#[test]
fn restored_backup_rewinds_journal_and_reapplies() {
    on_big_stack(|| {
        let fixture = Fixture::new();

        // Step 1: apply ordinal 1 on the source, THEN capture the snapshot
        // at exactly that durable state.
        let identity = fixture.runtime.block_on(bounded("source identity", async {
            fixture
                .db
                .system_store()
                .schema_migration_identity(DB_NAME)
                .await
                .expect("source identity")
        }));
        let ordinal1 = migration_body(1, "mig_items");
        let first = fixture.runtime.block_on(bounded(
            "apply ordinal 1",
            apply_migration(&fixture.db, &identity, 0, &ordinal1),
        ));
        assert_eq!(
            field(&first, "status").as_str(),
            Some("applied"),
            "ordinal 1 applies on the source"
        );
        let captured_status = fixture.runtime.block_on(bounded(
            "captured fence read",
            run_status(&fixture.db, &identity),
        ));
        let captured_fence = u64::try_from(int_field(&captured_status, "fence_generation"))
            .expect("fence generation fits u64");
        assert_eq!(int_field(&captured_status, "last_applied_ordinal"), 1);

        let _snapshot = fixture.capture_snapshot(false);

        // Step 2: a SECOND ordinal is applied AFTER the capture and proven
        // durable on the source — this is what the restore must unwind.
        let ordinal2 = migration_body(2, "mig_items_2");
        let second = fixture.runtime.block_on(bounded(
            "apply ordinal 2 after capture",
            apply_migration(&fixture.db, &identity, captured_fence, &ordinal2),
        ));
        assert_eq!(field(&second, "status").as_str(), Some("applied"));
        let after_second = fixture.runtime.block_on(bounded(
            "post-ordinal-2 status",
            run_status(&fixture.db, &identity),
        ));
        assert_eq!(int_field(&after_second, "last_applied_ordinal"), 2);
        assert!(
            fixture
                .db
                .get_db(DB_NAME)
                .unwrap()
                .has_table("main", "mig_items_2"),
            "the post-capture ordinal is durably applied AND effective on the source"
        );

        // Step 3: restore the EARLIER snapshot to the fresh target.
        let report: RestoreReport = run_restore(&fixture);
        assert!(report.files_restored > 0, "the restore copies files");

        let restored = fixture
            .runtime
            .block_on(bounded(
                "boot restored data dir",
                ShamirDb::init(SystemStoreConfig::Fjall(
                    fixture.target_data_dir().join(SYSTEM_DIR),
                )),
            ))
            .expect("boot restored engine");
        let restored_identity = fixture
            .runtime
            .block_on(bounded("restored identity", async {
                restored
                    .system_store()
                    .schema_migration_identity(DB_NAME)
                    .await
                    .expect("restored identity")
            }));
        assert_eq!(
            restored_identity, identity,
            "the stable database identity survives the restore"
        );
        let restored_status = fixture.runtime.block_on(bounded(
            "restored status",
            run_status(&restored, &restored_identity),
        ));
        assert_eq!(
            int_field(&restored_status, "last_applied_ordinal"),
            1,
            "the restored journal is REWOUND to the captured applied prefix"
        );
        let restored_fence = u64::try_from(int_field(&restored_status, "fence_generation"))
            .expect("fence generation fits u64");
        assert_eq!(
            restored_fence, captured_fence,
            "the fence generation is rewound to the captured released value"
        );
        assert!(
            restored
                .get_db(DB_NAME)
                .unwrap()
                .has_table("main", "mig_items"),
            "the captured prefix's effect exists"
        );
        assert!(
            !restored
                .get_db(DB_NAME)
                .unwrap()
                .has_table("main", "mig_items_2"),
            "the post-capture ordinal's effect is absent from the restore"
        );

        // Step 4: re-apply the IDENTICAL ordinal-2 body verbatim against the
        // rewound journal, with the status-derived expected fence generation.
        let reapplied = fixture.runtime.block_on(bounded(
            "re-apply ordinal 2 verbatim",
            apply_migration(&restored, &restored_identity, captured_fence, &ordinal2),
        ));
        assert_eq!(
            field(&reapplied, "status").as_str(),
            Some("applied"),
            "the rewound journal re-applies the previously applied ordinal verbatim"
        );
        assert!(
            restored
                .get_db(DB_NAME)
                .unwrap()
                .has_table("main", "mig_items_2"),
            "the re-applied ordinal's effect exists on the restored engine"
        );
        let final_status = fixture.runtime.block_on(bounded(
            "final restored status",
            run_status(&restored, &restored_identity),
        ));
        assert_eq!(int_field(&final_status, "last_applied_ordinal"), 2);

        drop(restored);
        drop(fixture);
    })
    .expect("test thread");
}
