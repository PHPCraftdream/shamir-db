//! Validator and function lifecycle e2e tests.

use serde_json::json;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::WriteOp;

use super::helpers::*;

// ═══════════════════════════════════════════════════════════════════════
// 1. create_validator over wire → bind → rejected insert → unbind → ok
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_validator_bind_reject_unbind_roundtrip() {
    let db = setup_db().await;

    // Step 1: create_validator over the wire
    let rejecting_wasm = make_wat_returning(&rejection_single_error());
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("v_reject").wasm(wasm_b64(&rejecting_wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &create_req).await.unwrap();
    let result = &resp.results["op"].records[0];
    assert_eq!(result["created_validator"], "v_reject");
    assert!(result.get("id").is_some(), "should return validator id");

    // Step 2: bind_validator over the wire
    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("v_reject", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let bind_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &bind_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["bound_validator"], "v_reject");

    // Step 3: insert should fail (validator rejects)
    let mut b = Batch::new();
    b.id("ins");
    b.insert(
        "ins",
        shamir_query_builder::write::insert("users")
            .row(shamir_query_builder::doc! { "name" => "Alice", "age" => 10 }),
    );
    let insert_req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &insert_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("too_young") || msg.contains("Validator"),
        "expected validation error, got: {msg}"
    );

    // Step 4: unbind_validator over the wire
    let mut b = Batch::new();
    b.id("ub");
    b.unbind_validator(
        "op",
        ddl::unbind_validator("v_reject", "users").db("testdb"),
    );
    let unbind_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &unbind_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Step 5: insert should now succeed
    let resp = db.execute("testdb", &insert_req).await;
    assert!(
        resp.is_ok(),
        "insert after unbind should succeed, got: {:?}",
        resp.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 2. create_function over wire → invoke / confirm in catalogue
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_function_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm(); // A no-op function that returns null
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("wire_echo").wasm(wasm_b64(&wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &create_req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0]["created_function"],
        "wire_echo"
    );

    // Verify the function is in the catalogue
    let functions = db.list_functions().await.unwrap();
    assert!(
        functions.contains(&"wire_echo".to_string()),
        "function should be in catalogue, got: {:?}",
        functions
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 3. drop_validator-while-bound → refused
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_validator_while_bound_refused() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("v_drop_test").wasm(wasm_b64(&wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    // Bind it
    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("v_drop_test", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let bind_req = b.to_request_via_msgpack();
    db.execute("testdb", &bind_req).await.unwrap();

    // Try to drop → should be refused
    let mut b = Batch::new();
    b.id("dv");
    b.drop_validator("op", ddl::drop_validator("v_drop_test"));
    let drop_req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &drop_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still bound") || msg.contains("cannot drop"),
        "expected still-bound error, got: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 4. create_function_folder over wire → succeeds
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_function_folder_over_wire() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["reports", "daily"]));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    let result = &resp.results["op"].records[0];
    assert_eq!(
        result["created_function_folder"],
        json!(["reports", "daily"])
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 5. name uniqueness: create_validator twice → error
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_validator_duplicate_rejected() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cv1");
    b.create_validator("op", ddl::create_validator("v_dup").wasm(wasm_b64(&wasm)));
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    // Second create with replace=false → should fail
    let err = db.execute("testdb", &create_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' error, got: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 6. drop_function over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_drop_test").wasm(wasm_b64(&wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    let mut b = Batch::new();
    b.id("df");
    b.drop_function("op", ddl::drop_function("fn_drop_test"));
    let drop_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Verify removed from catalogue
    let functions = db.list_functions().await.unwrap();
    assert!(
        !functions.contains(&"fn_drop_test".to_string()),
        "function should be removed from catalogue"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 7. rename_function over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn rename_function_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function("op", ddl::create_function("fn_old").wasm(wasm_b64(&wasm)));
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    let mut b = Batch::new();
    b.id("rf");
    b.rename_function("op", ddl::rename_function("fn_old", "fn_new"));
    let rename_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &rename_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["renamed_function"], "fn_old");
    assert_eq!(resp.results["op"].records[0]["to"], "fn_new");

    let functions = db.list_functions().await.unwrap();
    assert!(functions.contains(&"fn_new".to_string()));
    assert!(!functions.contains(&"fn_old".to_string()));
}

// ═══════════════════════════════════════════════════════════════════════
// 8. rename_validator over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn rename_validator_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator("op", ddl::create_validator("v_old").wasm(wasm_b64(&wasm)));
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    let mut b = Batch::new();
    b.id("rv");
    b.rename_validator("op", ddl::rename_validator("v_old", "v_new"));
    let rename_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &rename_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["renamed_validator"], "v_old");
    assert_eq!(resp.results["op"].records[0]["to"], "v_new");
}

// ═══════════════════════════════════════════════════════════════════════
// 9. list_validators over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn list_validators_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("v_list_test").wasm(wasm_b64(&wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    // Bind it
    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("v_list_test", "users")
            .db("testdb")
            .ops([WriteOp::Insert, WriteOp::Update])
            .priority(2000),
    );
    let bind_req = b.to_request_via_msgpack();
    db.execute("testdb", &bind_req).await.unwrap();

    // List
    let mut b = Batch::new();
    b.id("lv");
    b.list_validators("op", ddl::list_validators("users").db("testdb"));
    let list_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let result = &resp.results["op"].records[0];
    let validators = result["validators"].as_array().unwrap();
    assert!(
        !validators.is_empty(),
        "should have at least one validator binding"
    );
    assert_eq!(result["table"], "users");
}
