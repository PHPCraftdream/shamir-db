//! End-to-end tests for the S3/S5 validator pass on the write path.
//!
//! Tests that:
//! 1. An accepting validator allows the insert to succeed.
//! 2. Missing validator -> fail-closed error.
//! 3. Insert succeeds without any bindings (zero-cost fast path).
//! 4. A rejecting validator blocks the insert with field-bound error codes
//!    and the row is NOT persisted.
//! 5. A validator returning two errors surfaces both (single-validator
//!    multi-error).
//! 6. A validator with `stop: true` halts lower-priority validators.
//! 7. `drop_validator` is refused while the validator is bound.
//! 8. Ops filtering: a validator bound on `[Update]` does NOT fire on Insert.
//! 9. Fail-closed: a bound validator whose registry entry is missing aborts
//!    with `ValidatorInvalid`.

use serde_json::json;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// WAT helpers — build WASM modules that return baked msgpack bytes
// ═══════════════════════════════════════════════════════════════════════

/// WAT module that ignores input and returns msgpack `null` (0xC0) = valid.
const ACCEPT_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  ;; Return value area starts at byte 512.
  ;; We pre-populate memory[512] = 0xC0 (msgpack NULL).
  (data (i32.const 512) "\c0")

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  ;; Ignore the input; return (ptr=512, len=1) -> msgpack null -> valid.
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const 1)
    )
  )
)
"#;

fn accept_wasm() -> Vec<u8> {
    wat::parse_str(ACCEPT_WAT).expect("WAT parse failed")
}

/// Build a WAT module whose `shamir_call` returns the given `QueryValue`
/// serialised as msgpack. The data is placed at offset 512 in a 2-page
/// WASM memory and `shamir_call` returns `(ptr=512, len=N)`.
///
/// This is the WAT-baked-msgpack approach: always runs (no toolchain),
/// and the bytes are self-checking because they come from `rmp_serde`.
fn make_wat_returning(value: &QueryValue) -> Vec<u8> {
    let bytes = rmp_serde::to_vec(value).expect("msgpack encode");

    // Build WAT hex string from the bytes: "\xx\yy..."
    let hex_data: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
    let len = bytes.len();

    let wat = format!(
        r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  ;; Baked msgpack result at offset 512, {len} bytes.
  (data (i32.const 512) "{hex_data}")

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  ;; Ignore input; return pointer to the baked data.
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const {len})
    )
  )
)
"#
    );

    wat::parse_str(&wat).expect("generated WAT parse failed")
}

/// Build a `QueryValue` for a single-error rejection:
/// `{"errors":[{"field":["age"],"code":"too_young"}],"stop":false}`
fn rejection_single_error() -> QueryValue {
    let mut error_item = new_map();
    error_item.insert(
        "field".to_owned(),
        QueryValue::List(vec![QueryValue::Str("age".to_owned())]),
    );
    error_item.insert("code".to_owned(), QueryValue::Str("too_young".to_owned()));

    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(error_item)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(false));
    QueryValue::Map(root)
}

/// Build a `QueryValue` for a multi-error rejection (single validator,
/// two errors): field `["age"]` code `"too_young"` + field `["name"]`
/// code `"too_short"`.
fn rejection_multi_error() -> QueryValue {
    let mut e1 = new_map();
    e1.insert(
        "field".to_owned(),
        QueryValue::List(vec![QueryValue::Str("age".to_owned())]),
    );
    e1.insert("code".to_owned(), QueryValue::Str("too_young".to_owned()));

    let mut e2 = new_map();
    e2.insert(
        "field".to_owned(),
        QueryValue::List(vec![QueryValue::Str("name".to_owned())]),
    );
    e2.insert("code".to_owned(), QueryValue::Str("too_short".to_owned()));

    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(e1), QueryValue::Map(e2)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(false));
    QueryValue::Map(root)
}

/// Build a `QueryValue` for a stop-rejection:
/// `{"errors":[{"code":"fatal"}],"stop":true}`
fn rejection_stop() -> QueryValue {
    let mut error_item = new_map();
    error_item.insert("code".to_owned(), QueryValue::Str("fatal".to_owned()));

    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(error_item)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(true));
    QueryValue::Map(root)
}

/// Build a `QueryValue` for a rejection with a different code, used as
/// a lower-priority validator behind a `stop` validator.
fn rejection_secondary() -> QueryValue {
    let mut error_item = new_map();
    error_item.insert(
        "code".to_owned(),
        QueryValue::Str("should_not_appear".to_owned()),
    );

    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(error_item)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(false));
    QueryValue::Map(root)
}

// ═══════════════════════════════════════════════════════════════════════
// Self-check: the baked msgpack bytes round-trip correctly
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn baked_msgpack_roundtrips_correctly() {
    for (label, value) in [
        ("single", rejection_single_error()),
        ("multi", rejection_multi_error()),
        ("stop", rejection_stop()),
        ("secondary", rejection_secondary()),
    ] {
        let bytes = rmp_serde::to_vec(&value).expect("encode");
        let decoded: QueryValue = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(
            decoded, value,
            "baked msgpack for '{label}' should round-trip"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Setup helper
// ═══════════════════════════════════════════════════════════════════════

/// Helper: create an in-memory ShamirDb with "testdb/main/users" table.
async fn setup_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

/// Helper: build an insert batch request for one record into "users".
fn insert_request(id: &str, record: serde_json::Value) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.insert("ins", insert("users").row(record));
    b.to_request_via_msgpack()
}

/// Helper: build a select-all batch request for "users".
fn read_all_request(id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.query("all", Query::from("users"));
    b.to_request_via_msgpack()
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: accept validator allows insert (existing, carried forward)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn accept_validator_allows_insert() {
    let db = setup_db().await;

    db.create_validator_from_wasm("v_accept", &accept_wasm(), false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_accept",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    let request = insert_request("test_accept", json!({"name": "Alice"}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_ok(),
        "insert with an accepting validator should succeed, got: {:?}",
        response.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: missing validator -> fail-closed (existing, carried forward)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn missing_validator_fails_closed() {
    let db = setup_db().await;

    let id = db
        .create_validator_from_wasm("v_temp", &accept_wasm(), false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_temp",
        vec![WriteOp::Insert],
        1500,
    )
    .await
    .unwrap();

    // Forcibly remove the validator from the registry (simulates stale
    // binding after a deploy error).
    db.validators().remove(&id);

    let request = insert_request("test_missing", json!({"name": "Bob"}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_err(),
        "insert with a missing validator should fail"
    );
    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found in registry") || err_msg.contains("Validator invalid"),
        "error should mention the missing validator, got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: insert succeeds without bindings (existing, carried forward)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn insert_succeeds_without_bindings() {
    let db = setup_db().await;

    let request = insert_request("test_no_bindings", json!({"sku": "ABC123"}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_ok(),
        "insert without bindings should succeed, got: {:?}",
        response.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: REJECT e2e — real WAT validator with baked msgpack rejection
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn reject_validator_blocks_insert_with_field_error() {
    let db = setup_db().await;

    // Build a WAT validator that always returns a rejection with
    // field=["age"], code="too_young".
    let reject_wasm = make_wat_returning(&rejection_single_error());

    db.create_validator_from_wasm("v_reject", &reject_wasm, false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_reject",
        vec![WriteOp::Insert],
        5000,
    )
    .await
    .unwrap();

    // Attempt an insert — should fail with validator rejection.
    let request = insert_request("test_reject", json!({"name": "Eve", "age": 12}));
    let response = db.execute("testdb", &request).await;
    assert!(response.is_err(), "insert should be rejected by validator");

    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("too_young"),
        "error should contain the rejection code 'too_young', got: {err_msg}"
    );
    assert!(
        err_msg.contains("age"),
        "error should reference the field 'age', got: {err_msg}"
    );

    // Verify the row was NOT persisted — a follow-up read returns nothing.
    let read = read_all_request("test_reject_read");
    let read_resp = db.execute("testdb", &read).await.unwrap();
    let results = &read_resp.results["all"];
    assert!(
        results.records.is_empty(),
        "rejected row should NOT be persisted, got {} rows",
        results.records.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: multi-error from a single validator (collect-all equivalent)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn single_validator_multi_error() {
    let db = setup_db().await;

    let wasm = make_wat_returning(&rejection_multi_error());

    db.create_validator_from_wasm("v_multi", &wasm, false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_multi",
        vec![WriteOp::Insert],
        2000,
    )
    .await
    .unwrap();

    let request = insert_request("test_multi", json!({"name": "X", "age": 5}));
    let response = db.execute("testdb", &request).await;
    assert!(response.is_err(), "multi-error insert should be rejected");

    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("too_young"),
        "should contain 'too_young', got: {err_msg}"
    );
    assert!(
        err_msg.contains("too_short"),
        "should contain 'too_short', got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 6: stop=true halts lower-priority validators
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn stop_validator_halts_lower_priority() {
    let db = setup_db().await;

    // High-priority validator (priority 1000) returns stop=true + "fatal".
    let stop_wasm = make_wat_returning(&rejection_stop());
    db.create_validator_from_wasm("v_stop", &stop_wasm, false)
        .await
        .unwrap();

    // Low-priority validator (priority 9000) returns "should_not_appear".
    let secondary_wasm = make_wat_returning(&rejection_secondary());
    db.create_validator_from_wasm("v_secondary", &secondary_wasm, false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_stop",
        vec![WriteOp::Insert],
        1000,
    )
    .await
    .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_secondary",
        vec![WriteOp::Insert],
        9000,
    )
    .await
    .unwrap();

    let request = insert_request("test_stop", json!({"name": "Halt"}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_err(),
        "stop-validated insert should be rejected"
    );

    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("fatal"),
        "should contain the stop-validator's code 'fatal', got: {err_msg}"
    );
    assert!(
        !err_msg.contains("should_not_appear"),
        "the lower-priority validator's code should NOT appear (halted by stop), got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 7: drop_validator refused while bound (e2e through execute path)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_validator_refused_while_bound_e2e() {
    let db = setup_db().await;

    db.create_validator_from_wasm("v_bound", &accept_wasm(), false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_bound",
        vec![WriteOp::Insert],
        2000,
    )
    .await
    .unwrap();

    // Attempting to drop should fail because the validator is bound.
    let err = db.drop_validator("v_bound").await.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("still bound"),
        "drop should be refused with 'still bound', got: {err_msg}"
    );
    assert!(
        err_msg.contains("testdb/main/users"),
        "error should mention the bound table, got: {err_msg}"
    );

    // The validator should still be present.
    assert!(
        db.validator_id("v_bound").is_some(),
        "validator should still be registered"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 8: ops filtering — validator bound on [Update] does NOT fire on
//          Insert (insert succeeds even though the validator would reject)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ops_filtering_update_only_does_not_fire_on_insert() {
    let db = setup_db().await;

    // This validator would reject any write it fires on.
    let reject_wasm = make_wat_returning(&rejection_single_error());
    db.create_validator_from_wasm("v_update_only", &reject_wasm, false)
        .await
        .unwrap();

    // Bind on [Update] only — NOT Insert.
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_update_only",
        vec![WriteOp::Update],
        1500,
    )
    .await
    .unwrap();

    // Insert should succeed because the validator is not bound on Insert.
    let request = insert_request("test_ops_filter", json!({"name": "Allowed", "age": 5}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_ok(),
        "insert should succeed when validator is bound on [Update] only, got: {:?}",
        response.err()
    );

    // Verify the row IS persisted.
    let read = read_all_request("test_ops_filter_read");
    let read_resp = db.execute("testdb", &read).await.unwrap();
    let results = &read_resp.results["all"];
    assert_eq!(
        results.records.len(),
        1,
        "the inserted row should be persisted"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 9: fail-closed — bound validator whose registry entry is forcibly
//          removed -> write aborts with ValidatorInvalid
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fail_closed_bound_id_not_in_registry() {
    let db = setup_db().await;

    let id = db
        .create_validator_from_wasm("v_ephemeral", &accept_wasm(), false)
        .await
        .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_ephemeral",
        vec![WriteOp::Insert],
        3000,
    )
    .await
    .unwrap();

    // Forcibly remove from the registry to simulate a stale binding.
    db.validators().remove(&id);

    let request = insert_request("test_fail_closed", json!({"name": "Ghost"}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_err(),
        "insert should fail-closed when validator is missing from registry"
    );

    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found in registry") || err_msg.contains("Validator invalid"),
        "error should indicate fail-closed missing validator, got: {err_msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 10: collect-all with two distinct validators bound to the same
//           table — both rejection codes appear in the error
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn collect_all_two_validators_both_codes_present() {
    let db = setup_db().await;

    // Validator A: rejects with code "too_young" on field ["age"].
    let wasm_a = make_wat_returning(&rejection_single_error());
    db.create_validator_from_wasm("v_age", &wasm_a, false)
        .await
        .unwrap();

    // Validator B: rejects with code "too_short" on field ["name"].
    let mut e_b = new_map();
    e_b.insert(
        "field".to_owned(),
        QueryValue::List(vec![QueryValue::Str("name".to_owned())]),
    );
    e_b.insert("code".to_owned(), QueryValue::Str("too_short".to_owned()));

    let mut root_b = new_map();
    root_b.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(e_b)]),
    );
    root_b.insert("stop".to_owned(), QueryValue::Bool(false));

    let wasm_b = make_wat_returning(&QueryValue::Map(root_b));
    db.create_validator_from_wasm("v_name", &wasm_b, false)
        .await
        .unwrap();

    // Bind both on Insert with different priorities.
    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_age",
        vec![WriteOp::Insert],
        2000,
    )
    .await
    .unwrap();

    db.bind_validator(
        "testdb",
        "main",
        "users",
        "v_name",
        vec![WriteOp::Insert],
        3000,
    )
    .await
    .unwrap();

    let request = insert_request("test_collect_all", json!({"name": "X", "age": 5}));
    let response = db.execute("testdb", &request).await;
    assert!(
        response.is_err(),
        "insert violating both validators should be rejected"
    );

    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("too_young"),
        "should contain validator A's code 'too_young', got: {err_msg}"
    );
    assert!(
        err_msg.contains("too_short"),
        "should contain validator B's code 'too_short', got: {err_msg}"
    );
}
