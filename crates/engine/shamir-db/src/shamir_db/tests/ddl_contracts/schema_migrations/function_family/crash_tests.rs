//! Restart function-family migrations from real durable journal prefixes.

use base64::{engine::general_purpose::STANDARD, Engine};
use shamir_query_builder::ddl;
use shamir_query_types::admin::types::schema_migration::{SchemaMigrationBody, SchemaMigrationOp};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::WriteOp;

use super::super::helpers::{checksum, on_big_stack};
use super::helpers::{
    apply_request, body, bounded, create_function, create_validator, history, identity,
    persistent_owner_db, run_apply, run_op, DB, OWNER, REPO,
};
use crate::shamir_db::execute::admin_db_schema_migration::{test_capture_intent, test_verify};
use crate::shamir_db::system_store::{
    FormatEpoch, JournalEventKind, SchemaMigrationContext, SchemaMigrationStep, SystemStoreConfig,
};
use crate::types::value::QueryValue;
use crate::ShamirDb;

const NEXT: &str = "next_function";
const SOURCE: &str = "old_function";
const DESTINATION: &str = "new_function";

#[derive(Clone, Copy, Debug)]
enum CrashWindow {
    AfterIntent,
    AfterEffect,
    BetweenOperations,
}

/// Store the started event and operation 0 intent, optionally perform the
/// actual effect as OWNER, then optionally record completion. No live
/// runner or durability fault counters are involved.
async fn journal_prefix(
    db: &ShamirDb,
    migration: &SchemaMigrationBody,
    effect: Option<BatchOp>,
    window: CrashWindow,
) {
    let operation = &migration.operations[0];
    let evidence = if matches!(operation, SchemaMigrationOp::BindValidator(_)) {
        // Bind is deferred at admission; this intent is deliberately synthetic.
        assert!(matches!(window, CrashWindow::AfterIntent));
        None
    } else {
        Some(
            bounded(test_capture_intent(db, DB, OWNER, operation))
                .await
                .unwrap(),
        )
    };
    let target = bounded(identity(db)).await;
    let store = db.system_store();
    let context = SchemaMigrationContext::new(OWNER, FormatEpoch::Unknown);
    let metadata = migration.metadata.clone();
    let digest = checksum(migration);
    let generation = bounded(store.acquire_schema_migration_fence(
        target,
        0,
        metadata.clone(),
        migration.clone(),
        false,
        &context,
    ))
    .await
    .unwrap();
    assert_eq!(generation, 1);
    bounded(store.append_schema_migration_event(
        target,
        generation,
        metadata.clone(),
        digest,
        SchemaMigrationStep {
            kind: JournalEventKind::OperationIntent,
            operation_index: Some(0),
            evidence: evidence.clone(),
        },
        &context,
    ))
    .await
    .unwrap();
    if !matches!(window, CrashWindow::AfterIntent) {
        bounded(run_op(
            db,
            effect.expect("crash window requires a committed effect"),
        ))
        .await
        .unwrap();
    }
    if matches!(window, CrashWindow::BetweenOperations) {
        let completion = bounded(test_verify(db, DB, OWNER, operation, evidence.as_deref()))
            .await
            .unwrap();
        bounded(store.append_schema_migration_event(
            target,
            generation,
            metadata,
            digest,
            SchemaMigrationStep {
                kind: JournalEventKind::OperationComplete,
                operation_index: Some(0),
                evidence: Some(completion),
            },
            &context,
        ))
        .await
        .unwrap();
    }
}

async fn reopen(dir: &tempfile::TempDir, db: ShamirDb) -> ShamirDb {
    bounded(db.flush_all_for_test()).await.unwrap();
    drop(db);
    let db = bounded(ShamirDb::init(SystemStoreConfig::Fjall(
        dir.path().join("system"),
    )))
    .await
    .unwrap();
    assert_eq!(db.schema_migration_gate_in_doubt_generation(DB), Some(1));
    db
}

async fn function_row(db: &ShamirDb, name: &str) -> Option<QueryValue> {
    bounded(db.system_store().load_function(name))
        .await
        .unwrap()
}

async fn validator_row(db: &ShamirDb, name: &str) -> Option<QueryValue> {
    bounded(db.system_store().load_validator(name))
        .await
        .unwrap()
}

async fn assert_resumed(db: &ShamirDb, migration: &SchemaMigrationBody) {
    let target = bounded(identity(db)).await;
    let prior = bounded(history(db, target)).await;
    assert!(prior
        .events
        .iter()
        .all(|event| event.operation_index != Some(1)));
    assert!(bounded(function_row(db, NEXT)).await.is_none());
    let value = bounded(run_apply(db, apply_request(target, None, 1, migration)))
        .await
        .unwrap();
    assert_eq!(value["status"].as_str(), Some("applied"));
    assert!(bounded(function_row(db, NEXT)).await.is_some());
    let after = bounded(history(db, target)).await;
    assert!(after.events.starts_with(&prior.events));
    assert_eq!(after.applied.len(), 1);
    assert!(!after.in_doubt);
    for kind in [
        JournalEventKind::OperationIntent,
        JournalEventKind::OperationComplete,
    ] {
        let indices: Vec<_> = after
            .events
            .iter()
            .filter(|event| event.kind == kind)
            .map(|event| event.operation_index)
            .collect();
        assert_eq!(indices, [Some(0), Some(1)]);
    }
    assert!(db.schema_migration_gate_in_doubt_generation(DB).is_none());
}

#[test]
fn function_create_resumes_all_three_crash_windows() {
    on_big_stack(|| async {
        for window in [
            CrashWindow::AfterIntent,
            CrashWindow::AfterEffect,
            CrashWindow::BetweenOperations,
        ] {
            bounded(async {
                let (dir, db) = bounded(persistent_owner_db()).await;
                let create = create_function("created_function");
                let migration = body(
                    "create-function-crash",
                    1,
                    [create.clone(), create_function(NEXT)],
                );
                bounded(journal_prefix(&db, &migration, Some(create), window)).await;
                let db = bounded(reopen(&dir, db)).await;
                let before = bounded(function_row(&db, "created_function")).await;
                assert_eq!(before.is_none(), matches!(window, CrashWindow::AfterIntent));
                bounded(assert_resumed(&db, &migration)).await;
                let after = bounded(function_row(&db, "created_function"))
                    .await
                    .unwrap();
                assert_eq!(
                    after["wasm_b64"].as_str(),
                    Some(super::helpers::wasm().as_str())
                );
                if let Some(before) = before {
                    assert_eq!(
                        before, after,
                        "must keep the proven registration: {window:?}"
                    );
                }
            })
            .await;
        }
    });
}

#[test]
fn validator_create_resumes_all_three_crash_windows() {
    on_big_stack(|| async {
        for window in [
            CrashWindow::AfterIntent,
            CrashWindow::AfterEffect,
            CrashWindow::BetweenOperations,
        ] {
            bounded(async {
                let (dir, db) = bounded(persistent_owner_db()).await;
                let create = create_validator("created_validator");
                let migration = body(
                    "create-validator-crash",
                    1,
                    [create.clone(), create_function(NEXT)],
                );
                bounded(journal_prefix(&db, &migration, Some(create), window)).await;
                let db = bounded(reopen(&dir, db)).await;
                let before = bounded(validator_row(&db, "created_validator")).await;
                assert_eq!(before.is_none(), matches!(window, CrashWindow::AfterIntent));
                bounded(assert_resumed(&db, &migration)).await;
                let after = bounded(validator_row(&db, "created_validator"))
                    .await
                    .unwrap();
                assert_eq!(
                    after["wasm_b64"].as_str(),
                    Some(super::helpers::wasm().as_str())
                );
                assert!(db.validators().id_for_name("created_validator").is_some());
                if let Some(before) = before {
                    assert_eq!(
                        before, after,
                        "must keep the proven validator ID: {window:?}"
                    );
                }
            })
            .await;
        }
    });
}

#[test]
fn function_rename_resumes_all_three_crash_windows() {
    on_big_stack(|| async {
        for window in [
            CrashWindow::AfterIntent,
            CrashWindow::AfterEffect,
            CrashWindow::BetweenOperations,
        ] {
            bounded(async {
                let (dir, db) = bounded(persistent_owner_db()).await;
                bounded(run_op(&db, create_function(SOURCE))).await.unwrap();
                let original = bounded(function_row(&db, SOURCE)).await.unwrap();
                let rename = ddl::rename_function(SOURCE, DESTINATION);
                let migration = body(
                    "rename-function-crash",
                    1,
                    [rename.clone(), create_function(NEXT)],
                );
                bounded(journal_prefix(&db, &migration, Some(rename), window)).await;
                let db = bounded(reopen(&dir, db)).await;
                let before = bounded(function_row(&db, DESTINATION)).await;
                assert_eq!(before.is_none(), matches!(window, CrashWindow::AfterIntent));
                bounded(assert_resumed(&db, &migration)).await;
                assert!(bounded(function_row(&db, SOURCE)).await.is_none());
                let after = bounded(function_row(&db, DESTINATION)).await.unwrap();
                assert_eq!(after.get("wasm_b64"), original.get("wasm_b64"));
                if let Some(before) = before {
                    assert_eq!(before, after, "must not replay a proven rename: {window:?}");
                }
            })
            .await;
        }
    });
}

#[test]
fn completed_function_with_replaced_code_refuses_resume_without_replay() {
    on_big_stack(|| async {
        let (dir, db) = bounded(persistent_owner_db()).await;
        let create = create_function("drifted_function");
        let migration = body(
            "function-code-drift",
            1,
            [create.clone(), create_function(NEXT)],
        );
        bounded(journal_prefix(
            &db,
            &migration,
            Some(create),
            CrashWindow::BetweenOperations,
        ))
        .await;

        let changed = STANDARD.encode(
            wat::parse_str(
                r#"(module
        (memory (export "memory") 2)
        (func (export "shamir_alloc") (param i32) (result i32) i32.const 2048)
        (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0)
    )"#,
            )
            .unwrap(),
        );
        // The embedded executor consumes the ordinary admin operation; the
        // server's transport HMAC verification is outside this fixture.
        let replacement = ddl::create_function("drifted_function")
            .wasm(changed.as_str())
            .replace()
            .hmac("00".repeat(32))
            .build()
            .unwrap();
        bounded(run_op(&db, replacement)).await.unwrap();
        let db = bounded(reopen(&dir, db)).await;
        let drifted = bounded(function_row(&db, "drifted_function"))
            .await
            .unwrap();
        assert_eq!(drifted["wasm_b64"].as_str(), Some(changed.as_str()));
        let target = bounded(identity(&db)).await;
        let prior = bounded(history(&db, target)).await;
        let error = bounded(run_apply(&db, apply_request(target, None, 1, &migration)))
            .await
            .expect_err("completion must not hide changed code");
        assert_eq!(
            error.code(),
            Some("schema_migration_postcondition_failed"),
            "{error}"
        );
        assert!(error.to_string().contains("durably completed"), "{error}");
        assert_eq!(
            bounded(function_row(&db, "drifted_function")).await,
            Some(drifted)
        );
        assert!(bounded(function_row(&db, NEXT)).await.is_none());
        let after = bounded(history(&db, target)).await;
        assert!(after.events.starts_with(&prior.events));
        let added: Vec<_> = after.events[prior.events.len()..]
            .iter()
            .map(|event| event.kind)
            .collect();
        assert_eq!(added, [JournalEventKind::RecoveryAcquired]);
    });
}

#[test]
fn ambiguous_pending_function_rename_stays_in_doubt_without_next_effect() {
    on_big_stack(|| async {
        let (dir, db) = bounded(persistent_owner_db()).await;
        bounded(run_op(&db, create_function(SOURCE))).await.unwrap();
        let migration = body(
            "ambiguous-rename",
            1,
            [
                ddl::rename_function(SOURCE, DESTINATION),
                create_function(NEXT),
            ],
        );
        bounded(journal_prefix(
            &db,
            &migration,
            None,
            CrashWindow::AfterIntent,
        ))
        .await;
        // Both names carry matching code, but the source still exists, so the
        // intended rename cannot be proven and must not be retried.
        bounded(run_op(&db, create_function(DESTINATION)))
            .await
            .unwrap();
        let db = bounded(reopen(&dir, db)).await;
        let source = bounded(function_row(&db, SOURCE)).await.unwrap();
        let destination = bounded(function_row(&db, DESTINATION)).await.unwrap();
        let target = bounded(identity(&db)).await;
        for generation in [1, 2] {
            let error = bounded(run_apply(
                &db,
                apply_request(target, None, generation, &migration),
            ))
            .await
            .expect_err("ambiguous rename cannot advance");
            assert_eq!(error.code(), Some("schema_migration_in_doubt"), "{error}");
            assert_eq!(
                db.schema_migration_gate_in_doubt_generation(DB),
                Some(generation + 1)
            );
            assert_eq!(
                bounded(function_row(&db, SOURCE)).await.as_ref(),
                Some(&source)
            );
            assert_eq!(
                bounded(function_row(&db, DESTINATION)).await.as_ref(),
                Some(&destination)
            );
            assert!(bounded(function_row(&db, NEXT)).await.is_none());
            let after = bounded(history(&db, target)).await;
            assert!(after.in_doubt);
            assert!(after.applied.is_empty());
            assert_eq!(after.current_status, Some(JournalEventKind::InDoubt));
            assert!(after
                .events
                .iter()
                .all(|event| event.operation_index != Some(1)
                    && event.kind != JournalEventKind::OperationComplete));
            assert_eq!(after.events.last().unwrap().operation_index, Some(0));
        }
    });
}

#[test]
fn synthetic_pending_bind_is_deferred_before_replay_and_next_effect() {
    on_big_stack(|| async {
        let (dir, db) = bounded(persistent_owner_db()).await;
        bounded(run_op(&db, ddl::create_table("items").repo(REPO).build()))
            .await
            .unwrap();
        bounded(run_op(&db, create_validator("row_guard")))
            .await
            .unwrap();
        let bind = ddl::bind_validator("row_guard", "items")
            .db(DB)
            .repo(REPO)
            .ops([WriteOp::Insert])
            .build();
        let migration = body("pending-bind", 1, [bind, create_function(NEXT)]);
        bounded(journal_prefix(
            &db,
            &migration,
            None,
            CrashWindow::AfterIntent,
        ))
        .await;
        let db = bounded(reopen(&dir, db)).await;
        let target = bounded(identity(&db)).await;
        let prior = bounded(history(&db, target)).await;
        let error = bounded(run_apply(&db, apply_request(target, None, 1, &migration)))
            .await
            .expect_err("even a journaled bind is deferred");
        assert!(
            error
                .to_string()
                .contains("schema_migration_operation_deferred:"),
            "{error}"
        );
        assert_eq!(bounded(history(&db, target)).await, prior);
        assert_eq!(db.schema_migration_gate_in_doubt_generation(DB), Some(1));
        bounded(db.check_seed_catalogue()).await.unwrap();
        assert!(bounded(db.list_validator_bindings(DB, REPO, "items"))
            .await
            .unwrap()
            .is_empty());
        assert!(bounded(function_row(&db, NEXT)).await.is_none());
    });
}
