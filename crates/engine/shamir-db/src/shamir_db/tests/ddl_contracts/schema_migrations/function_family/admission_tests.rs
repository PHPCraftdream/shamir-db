use shamir_query_builder::{batch::Batch, ddl, write::Insert, SchemaMigration};
use shamir_query_types::admin::types::schema_migration::SchemaMigrationFunctionDependencies;
use shamir_types::types::{common::new_map, record_id::RecordId};

use super::helpers::{
    apply_request, body, bounded, create_function, create_validator, history, identity,
    persistent_owner_db, run_apply, run_batch, run_op, run_plan, wasm, DB, REPO,
};
use crate::query::batch::BatchError;
use crate::types::value::QueryValue;

fn message(error: &BatchError) -> &str {
    match error {
        BatchError::QueryError { message, .. } => message,
        other => panic!("expected a query refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_create_and_explicit_check_share_plan_and_apply_preflight_without_effects() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    let pristine = history(&db, target).await;
    // Valid base64, invalid WASM; no external compiler/toolchain needed.
    let invalid = "AA==";
    let check = ddl::check_function().wasm(invalid).build().unwrap();
    let direct = run_op(&db, check.clone())
        .await
        .expect_err("invalid WASM must fail admission");
    for operation in [
        ddl::create_function("broken")
            .wasm(invalid)
            .build()
            .unwrap(),
        check,
    ] {
        // A valid effect first proves the later bad op is preflighted, not
        // discovered after execution already wrote intent/catalogue state.
        let migration = body(
            "invalid-code",
            1,
            [create_function("valid_first"), operation],
        );
        let plan = run_plan(&db, target, &migration)
            .await
            .expect_err("plan must preflight code");
        let apply = run_apply(&db, apply_request(target, None, 0, &migration))
            .await
            .expect_err("apply must preflight code");
        for error in [&plan, &apply] {
            assert_eq!(error.code(), direct.code());
            assert_eq!(message(error), message(&direct));
        }
        assert_eq!(history(&db, target).await, pristine);
        assert!(db.schema_migration_gate_generation(DB).is_none());
        for name in ["valid_first", "broken"] {
            assert!(bounded(db.system_store().load_function(name))
                .await
                .unwrap()
                .is_none());
        }
    }
}

#[tokio::test]
async fn hmac_gated_drops_and_replacements_are_refused_before_intent() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    run_op(&db, create_function("greeter")).await.unwrap();
    run_op(&db, create_validator("row_guard")).await.unwrap();
    let function = bounded(db.system_store().load_function("greeter"))
        .await
        .unwrap()
        .unwrap();
    let validator = bounded(db.system_store().load_validator("row_guard"))
        .await
        .unwrap()
        .unwrap();
    let pristine = history(&db, target).await;
    // Inner tags cannot authorize an operation covered by the outer migration.
    let tag = "00".repeat(32);
    for operation in [
        ddl::drop_function("greeter")
            .hmac(tag.as_str())
            .build()
            .unwrap(),
        ddl::drop_validator("row_guard")
            .hmac(tag.as_str())
            .build()
            .unwrap(),
        ddl::create_function("greeter")
            .wasm(wasm())
            .replace()
            .hmac(tag.as_str())
            .build()
            .unwrap(),
        ddl::create_validator("row_guard")
            .wasm(wasm())
            .replace()
            .hmac(tag.as_str())
            .build()
            .unwrap(),
    ] {
        let migration = body("needs-hmac", 1, [operation]);
        let error = run_apply(&db, apply_request(target, None, 0, &migration))
            .await
            .expect_err("migration cannot authorize HMAC-required operations");
        assert_eq!(error.code(), Some("hmac_required"), "{error}");
        assert_eq!(history(&db, target).await, pristine);
        assert!(db.schema_migration_gate_generation(DB).is_none());
        assert_eq!(
            bounded(db.system_store().load_function("greeter"))
                .await
                .unwrap()
                .as_ref(),
            Some(&function)
        );
        assert_eq!(
            bounded(db.system_store().load_validator("row_guard"))
                .await
                .unwrap()
                .as_ref(),
            Some(&validator)
        );
    }
}

#[tokio::test]
async fn plan_shows_declared_dependents_and_apply_refuses_unresolved_endpoint() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    run_op(&db, create_function("greeter")).await.unwrap();
    let pristine = history(&db, target).await;
    let migration = SchemaMigration::new("dependency-review", 1, "review author dependencies")
        .dependencies(vec![SchemaMigrationFunctionDependencies {
            function: "greeter".into(),
            depends_on: vec!["missing_callee".into()],
        }])
        .op(ddl::alter_function("greeter", "never"))
        .unwrap()
        .build()
        .unwrap();
    let original = bounded(db.system_store().load_function("greeter"))
        .await
        .unwrap()
        .unwrap();
    let plan = run_plan(&db, target, &migration).await.unwrap();
    let summary = plan["recovery_summary"].as_str().unwrap();
    let QueryValue::List(operations) = &plan["operations"] else {
        panic!("expected planned operations")
    };
    assert_eq!(operations.len(), 1);
    assert_eq!(
        operations[0]["dependent_functions"],
        QueryValue::List(vec![QueryValue::Str("greeter".into())])
    );
    assert!(summary.contains("apply is blocked by unresolved function dependencies"));
    assert!(summary.contains(r#""greeter" depends on ["missing_callee"]"#));
    let error = run_apply(&db, apply_request(target, None, 0, &migration))
        .await
        .expect_err("unresolved dependency must block apply");
    assert_eq!(
        error.code(),
        Some("schema_migration_postcondition_failed"),
        "{error}"
    );
    assert!(
        message(&error).contains("'missing_callee' could not be resolved"),
        "{error}"
    );
    assert_eq!(history(&db, target).await, pristine);
    assert!(db.schema_migration_gate_generation(DB).is_none());
    assert_eq!(
        bounded(db.system_store().load_function("greeter"))
            .await
            .unwrap(),
        Some(original)
    );
}

#[tokio::test]
async fn bind_on_a_populated_table_is_deferred_without_binding_or_history_changes() {
    let (_dir, db) = persistent_owner_db().await;
    let target = identity(&db).await;
    run_op(&db, ddl::create_table("items").repo(REPO).build())
        .await
        .unwrap();
    run_op(&db, create_validator("row_guard")).await.unwrap();
    bounded(db.check_seed_catalogue()).await.unwrap();
    let mut row = new_map();
    row.insert("_id".into(), QueryValue::Str(RecordId::new().to_string()));
    row.insert("amount".into(), QueryValue::Int(-1));
    let mut batch = Batch::new();
    batch.insert(
        "seed",
        Insert::with_repo(REPO, "items").row(QueryValue::Map(row)),
    );
    let inserted = run_batch(&db, batch).await.unwrap();
    assert_eq!(inserted.results["seed"].records.len(), 1);
    assert!(bounded(db.list_validator_bindings(DB, REPO, "items"))
        .await
        .unwrap()
        .is_empty());
    let pristine = history(&db, target).await;

    // The gate doesn't drain writes staged before it was acquired, so even
    // a table scan would not safely establish the new binding's invariant.
    let migration = body(
        "deferred-bind",
        1,
        [ddl::bind_validator("row_guard", "items")
            .db(DB)
            .repo(REPO)
            .ops([ddl::WriteOp::Insert])
            .build()],
    );
    let error = run_apply(&db, apply_request(target, None, 0, &migration))
        .await
        .expect_err("bind must not activate a validator under this fence");
    assert_eq!(error.code(), None, "{error}");
    assert!(
        message(&error).starts_with("schema_migration_operation_deferred: "),
        "{error}"
    );
    assert!(
        message(&error).contains("'row_guard' to table 'testdb.main.items'"),
        "{error}"
    );
    assert!(bounded(db.list_validator_bindings(DB, REPO, "items"))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(history(&db, target).await, pristine);
    assert!(db.schema_migration_gate_generation(DB).is_none());
}
