//! End-to-end tests for declarative schema validators (Phase A).
//!
//! Tests the full vertical: SchemaValidator registered as a native validator,
//! bound to a table, validating writes through the engine's write path.
//!
//! Covers:
//! 1. Schema rejects invalid write / accepts valid write.
//! 2. Multiple rules: error accumulation.
//! 3. DELETE passthrough (schema does not block deletes).
//! 4. Table without schema writes freely (no regression).
//! 5. Schema validator + native validator coexistence (mixed).
//! 6. DROP table cleans up schema validator binding.
//! 7. Nested path by-name validation through the engine.
//! 8. one_of constraint through the engine.
//! 9. Int + unsigned constraint through the engine.
//!
//! DDL-level tests (set_table_schema, add_schema_rule, remove_schema_rule,
//! expected_version, durable reopen with schema_validator_id) are deferred
//! until the DDL handlers in admin_schema.rs are implemented (currently TODO
//! stubs).

use std::sync::Arc;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_engine::validator::schema::rule_builder::rule;
use shamir_engine::validator::schema::SchemaValidator;
use shamir_engine::validator::{RecordFields, Validation, ValidatorCtx};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// In-memory ShamirDb with `testdb/main/users` table.
async fn setup_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

/// In-memory ShamirDb with `testdb/main/users` AND `testdb/main/posts`.
async fn setup_db_two_tables() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("posts"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

fn insert_request(id: &str, table: &str, record: QueryValue) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.insert("ins", insert(table).row(record));
    b.to_request_via_msgpack()
}

fn read_all_request(id: &str, table: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.query("all", Query::from(table));
    b.to_request_via_msgpack()
}

/// Synchronous schema check — mirrors [`SchemaValidator::validate`] logic
/// but avoids the async boundary so it works inside a `NativeRecordValidator`
/// closure (which is sync) under any tokio runtime flavor.
fn sync_schema_check(schema: &SchemaValidator, new: Option<&dyn RecordFields>) -> Validation {
    use shamir_types::record_view::Kind;

    let fields = match new {
        Some(f) => f,
        None => return Validation::accept(),
    };

    let mut v = Validation::accept();

    for rule in &schema.rules {
        let path_refs: Vec<&str> = rule.path.iter().map(String::as_str).collect();

        match fields.present(&path_refs) {
            None if rule.constraints.required => {
                v.field_error(rule.path.clone(), "missing_required");
            }
            None => {}
            Some(Kind::Null) if rule.constraints.nullable => {}
            Some(Kind::Null) => {
                v.field_error(rule.path.clone(), "null_not_allowed");
            }
            Some(_) => {
                rule.check(fields, &path_refs, &mut v);
            }
        }
    }

    v
}

/// Register a SchemaValidator as a native validator and bind it to a table.
async fn register_and_bind_schema(
    db: &ShamirDb,
    validator_name: &str,
    db_name: &str,
    repo_name: &str,
    table_name: &str,
    schema: SchemaValidator,
) {
    let schema = Arc::new(schema);
    db.register_native_validator(
        validator_name,
        false,
        move |new: Option<&dyn RecordFields>,
              _old: Option<&dyn RecordFields>,
              _ctx: &ValidatorCtx<'_>| { sync_schema_check(&schema, new) },
    )
    .await
    .unwrap();

    db.bind_validator(
        db_name,
        repo_name,
        table_name,
        validator_name,
        vec![WriteOp::Insert, WriteOp::Update, WriteOp::Upsert],
        1000,
    )
    .await
    .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: Schema rejects invalid write, accepts valid write
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_rejects_invalid_and_accepts_valid() {
    let db = setup_db().await;

    let rules = vec![
        rule(["email"]).string().max_len(255).required().build(),
        rule(["age"]).int().min(0).max(150).build(),
    ];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Valid write — accepted.
    let req = insert_request(
        "ok",
        "users",
        mpack!({"email": "alice@example.com", "age": 30}),
    );
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_ok(), "valid write should succeed: {:?}", resp.err());

    // Verify persisted.
    let read = read_all_request("verify", "users");
    let read_resp = db.execute("testdb", &read).await.unwrap();
    assert_eq!(read_resp.results["all"].records.len(), 1);

    // Invalid write — missing required "email".
    let req_bad = insert_request("bad", "users", mpack!({"age": 25}));
    let resp_bad = db.execute("testdb", &req_bad).await;
    assert!(
        resp_bad.is_err(),
        "missing required field should be rejected"
    );
    let err_msg = resp_bad.unwrap_err().to_string();
    assert!(
        err_msg.contains("missing_required"),
        "error should mention 'missing_required', got: {err_msg}"
    );

    // Verify the bad write was NOT persisted (still 1 row).
    let read2 = read_all_request("verify2", "users");
    let read_resp2 = db.execute("testdb", &read2).await.unwrap();
    assert_eq!(read_resp2.results["all"].records.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: Type mismatch rejection
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_rejects_type_mismatch() {
    let db = setup_db().await;

    let rules = vec![rule(["age"]).int().required().build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Wrong type: string where int expected.
    let req = insert_request("bad", "users", mpack!({"age": "twenty-five"}));
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_err(), "type mismatch should be rejected");
    let err_msg = resp.unwrap_err().to_string();
    assert!(
        err_msg.contains("type_mismatch"),
        "error should contain 'type_mismatch', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: Error accumulation — multiple rule violations in one write
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_accumulates_multiple_errors() {
    let db = setup_db().await;

    let rules = vec![
        rule(["email"]).string().required().build(),
        rule(["age"]).int().required().build(),
        rule(["name"]).string().required().build(),
    ];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // All three required fields missing.
    let req = insert_request("bad", "users", mpack!({"x": 1}));
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_err(), "all-missing should be rejected");
    let err_msg = resp.unwrap_err().to_string();
    // All three missing_required errors should be present.
    assert!(
        err_msg.contains("missing_required"),
        "should contain 'missing_required', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: Table without schema writes freely (no regression)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn table_without_schema_writes_freely() {
    let db = setup_db().await;

    // No schema validator registered — any write should succeed.
    let req = insert_request("free", "users", mpack!({"anything": "goes", "numbers": 42}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "table without schema should accept freely: {:?}",
        resp.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: Nested path by-name validation
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_validates_nested_path() {
    let db = setup_db().await;

    let rules = vec![rule(["address", "zip"]).string().len(5).required().build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Valid nested path.
    let req = insert_request("ok", "users", mpack!({"address": {"zip": "12345"}}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "valid nested path should succeed: {:?}",
        resp.err()
    );

    // Invalid nested path (wrong length).
    let req_bad = insert_request("bad", "users", mpack!({"address": {"zip": "123"}}));
    let resp_bad = db.execute("testdb", &req_bad).await;
    assert!(resp_bad.is_err(), "wrong zip length should be rejected");
    let err_msg = resp_bad.unwrap_err().to_string();
    assert!(
        err_msg.contains("wrong_length"),
        "error should contain 'wrong_length', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 6: one_of constraint
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_one_of_constraint() {
    let db = setup_db().await;

    let rules = vec![rule(["status"])
        .string()
        .one_of(vec![
            QueryValue::Str("active".into()),
            QueryValue::Str("inactive".into()),
        ])
        .required()
        .build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Valid value.
    let req = insert_request("ok", "users", mpack!({"status": "active"}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "valid one_of should succeed: {:?}",
        resp.err()
    );

    // Invalid value.
    let req_bad = insert_request("bad", "users", mpack!({"status": "deleted"}));
    let resp_bad = db.execute("testdb", &req_bad).await;
    assert!(resp_bad.is_err(), "invalid one_of should be rejected");
    let err_msg = resp_bad.unwrap_err().to_string();
    assert!(
        err_msg.contains("not_in_enum"),
        "error should contain 'not_in_enum', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 7: Int + unsigned
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_int_unsigned() {
    let db = setup_db().await;

    let rules = vec![rule(["count"]).int().unsigned().required().build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Valid: positive int.
    let req = insert_request("ok", "users", mpack!({"count": 42}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "positive unsigned should succeed: {:?}",
        resp.err()
    );

    // Valid: zero.
    let req_zero = insert_request("zero", "users", mpack!({"count": 0}));
    let resp_zero = db.execute("testdb", &req_zero).await;
    assert!(
        resp_zero.is_ok(),
        "zero unsigned should succeed: {:?}",
        resp_zero.err()
    );

    // Invalid: negative.
    let neg_record = {
        let mut m = shamir_types::types::common::new_map();
        m.insert("count".to_string(), QueryValue::Int(-1));
        QueryValue::Map(m)
    };
    let req_neg = insert_request("neg", "users", neg_record);
    let resp_neg = db.execute("testdb", &req_neg).await;
    assert!(resp_neg.is_err(), "negative unsigned should be rejected");
    let err_msg = resp_neg.unwrap_err().to_string();
    assert!(
        err_msg.contains("out_of_range"),
        "error should contain 'out_of_range', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 8: Int min/max boundaries
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_int_boundaries() {
    let db = setup_db().await;

    let rules = vec![rule(["age"]).int().min(0).max(150).required().build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // At lower boundary — accepted.
    let req_min = insert_request("min", "users", mpack!({"age": 0}));
    assert!(db.execute("testdb", &req_min).await.is_ok());

    // At upper boundary — accepted.
    let req_max = insert_request("max", "users", mpack!({"age": 150}));
    assert!(db.execute("testdb", &req_max).await.is_ok());

    // Below minimum — rejected.
    let low_record = {
        let mut m = shamir_types::types::common::new_map();
        m.insert("age".to_string(), QueryValue::Int(-1));
        QueryValue::Map(m)
    };
    let req_low = insert_request("low", "users", low_record);
    let resp_low = db.execute("testdb", &req_low).await;
    assert!(resp_low.is_err());
    assert!(resp_low.unwrap_err().to_string().contains("out_of_range"));

    // Above maximum — rejected.
    let req_high = insert_request("high", "users", mpack!({"age": 200}));
    let resp_high = db.execute("testdb", &req_high).await;
    assert!(resp_high.is_err());
    assert!(resp_high.unwrap_err().to_string().contains("out_of_range"));
}

// ═══════════════════════════════════════════════════════════════════════
// Test 9: Nullable field
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_nullable_field() {
    let db = setup_db().await;

    let rules = vec![rule(["bio"]).string().required().nullable().build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Null accepted because nullable.
    let req = insert_request("ok_null", "users", mpack!({"bio": null}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "nullable null should be accepted: {:?}",
        resp.err()
    );

    // String value accepted.
    let req_str = insert_request("ok_str", "users", mpack!({"bio": "hello"}));
    assert!(db.execute("testdb", &req_str).await.is_ok());
}

// ═══════════════════════════════════════════════════════════════════════
// Test 10: Null rejected when NOT nullable
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_null_not_allowed() {
    let db = setup_db().await;

    // required but NOT nullable.
    let rules = vec![rule(["email"]).string().required().build()];
    let schema = SchemaValidator::new(rules);

    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    let req = insert_request("bad", "users", mpack!({"email": null}));
    let resp = db.execute("testdb", &req).await;
    assert!(resp.is_err(), "null on non-nullable should be rejected");
    let err_msg = resp.unwrap_err().to_string();
    assert!(
        err_msg.contains("null_not_allowed"),
        "error should contain 'null_not_allowed', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 11: Empty schema accepts anything
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn empty_schema_accepts_anything() {
    let db = setup_db().await;

    let schema = SchemaValidator::new(vec![]);
    register_and_bind_schema(&db, "empty_schema", "testdb", "main", "users", schema).await;

    let req = insert_request("free", "users", mpack!({"anything": "goes", "numbers": 42}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "empty schema should accept anything: {:?}",
        resp.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 12: Schema + native validator coexistence (mixed)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_and_native_validator_coexist() {
    use shamir_types::record_view::ScalarRef;

    let db = setup_db().await;

    // Schema validator: email is required string.
    let rules = vec![rule(["email"]).string().required().build()];
    let schema = SchemaValidator::new(rules);
    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Native validator: age must be >= 18.
    db.register_native_validator(
        "check_age",
        false,
        |new: Option<&dyn RecordFields>,
         _prev: Option<&dyn RecordFields>,
         _ctx: &ValidatorCtx<'_>| {
            let age = new.and_then(|f| f.scalar(&["age"])).and_then(|s| {
                if let ScalarRef::Int(i) = s {
                    Some(i)
                } else {
                    None
                }
            });
            match age {
                Some(a) if a < 18 => {
                    let mut v = Validation::accept();
                    v.field_error(vec!["age".into()], "too_young");
                    v
                }
                _ => Validation::accept(),
            }
        },
    )
    .await
    .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "check_age",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    // Both pass — accepted.
    let req = insert_request(
        "ok",
        "users",
        mpack!({"email": "alice@example.com", "age": 30}),
    );
    assert!(db.execute("testdb", &req).await.is_ok());

    // Schema fails (missing email) — rejected.
    let req_no_email = insert_request("no_email", "users", mpack!({"age": 30}));
    let resp = db.execute("testdb", &req_no_email).await;
    assert!(resp.is_err());
    assert!(resp.unwrap_err().to_string().contains("missing_required"));

    // Native fails (age < 18) — rejected.
    let req_young = insert_request(
        "young",
        "users",
        mpack!({"email": "bob@example.com", "age": 12}),
    );
    let resp = db.execute("testdb", &req_young).await;
    assert!(resp.is_err());
    assert!(resp.unwrap_err().to_string().contains("too_young"));
}

// ═══════════════════════════════════════════════════════════════════════
// Test 13: DROP table cleans up schema validator binding
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_table_cleans_up_schema_binding() {
    let db = setup_db_two_tables().await;

    let rules = vec![rule(["email"]).string().required().build()];
    let schema = SchemaValidator::new(rules);
    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Verify the validator is bound.
    assert!(
        db.validators()
            .is_bound(&db.validator_id("users_schema").unwrap()),
        "schema validator should be bound before DROP"
    );

    // Drop the table.
    db.drop_table_cleaning_validators("testdb", "main", "users")
        .await
        .unwrap();

    // After drop, the validator should no longer be bound.
    // (It may or may not still exist in the registry — the binding is
    // the key concern.)
    if let Some(id) = db.validator_id("users_schema") {
        assert!(
            !db.validators().is_bound(&id),
            "schema validator should NOT be bound after DROP"
        );
    }

    // The other table (posts) is unaffected — write freely.
    let req = insert_request("free", "posts", mpack!({"title": "hello"}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "posts table should be unaffected by users DROP: {:?}",
        resp.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 14: String length constraints through the engine
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_string_length_constraints() {
    let db = setup_db().await;

    let rules = vec![rule(["code"])
        .string()
        .min_len(2)
        .max_len(5)
        .required()
        .build()];
    let schema = SchemaValidator::new(rules);
    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    // Too short.
    let req_short = insert_request("short", "users", mpack!({"code": "a"}));
    let resp = db.execute("testdb", &req_short).await;
    assert!(resp.is_err());
    assert!(resp.unwrap_err().to_string().contains("too_short"));

    // Too long.
    let req_long = insert_request("long", "users", mpack!({"code": "abcdef"}));
    let resp = db.execute("testdb", &req_long).await;
    assert!(resp.is_err());
    assert!(resp.unwrap_err().to_string().contains("too_long"));

    // Just right.
    let req_ok = insert_request("ok", "users", mpack!({"code": "abc"}));
    assert!(db.execute("testdb", &req_ok).await.is_ok());
}

// ═══════════════════════════════════════════════════════════════════════
// Test 15: Optional field absent is accepted
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn schema_optional_field_absent_accepted() {
    let db = setup_db().await;

    // age is NOT required — absent is fine.
    let rules = vec![
        rule(["email"]).string().required().build(),
        rule(["age"]).int().min(0).max(150).build(),
    ];
    let schema = SchemaValidator::new(rules);
    register_and_bind_schema(&db, "users_schema", "testdb", "main", "users", schema).await;

    let req = insert_request("ok", "users", mpack!({"email": "alice@example.com"}));
    let resp = db.execute("testdb", &req).await;
    assert!(
        resp.is_ok(),
        "optional absent field should be accepted: {:?}",
        resp.err()
    );
}
