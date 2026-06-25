//! Phase E.3 (#243) — DropFunction-as-validator guard.
//!
//! Verifies the server-side guard in `handle_drop_function`: dropping a
//! function whose name collides with a *bound* validator is refused with
//! code `drop_refused_bound` and an informative message listing the bound
//! tables. After the validator is unbound (or dropped), the function drop
//! succeeds.
//!
//! Functions and validators live in independent registries but share the
//! `FunctionNamespace`; the guard treats a name that resolves to a bound
//! validator as a live reference that would dangle if the function were
//! removed.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::WriteOp;

use super::helpers::*;

// ═══════════════════════════════════════════════════════════════════════
// drop_function refused while a same-named validator is bound
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_refused_when_validator_bound() {
    let db = setup_db().await;

    let wasm = accept_wasm();

    // 1. Create a function named "shared_name".
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // 2. Create a validator with the SAME name (independent registry,
    //    same FunctionNamespace — this is the collision the guard catches).
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // 3. Bind the validator to the "users" table.
    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("shared_name", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // 4. Drop the FUNCTION → must be refused with drop_refused_bound.
    let mut b = Batch::new();
    b.id("df");
    b.drop_function("op", ddl::drop_function("shared_name"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("drop_refused_bound"),
        "expected code 'drop_refused_bound', got: {:?} ({})",
        err.code(),
        err
    );
    let msg = err.to_string();
    assert!(
        msg.contains("shared_name"),
        "error should mention the function name, got: {msg}"
    );
    assert!(
        msg.contains("testdb/main/users"),
        "error should list the bound table, got: {msg}"
    );

    // The function must still exist (guard did not remove it).
    let functions = db.list_functions().await.unwrap();
    assert!(
        functions.contains(&"shared_name".to_string()),
        "function must still exist after refused drop"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// After unbinding the validator, the function drop succeeds (existed: true)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_succeeds_after_unbind() {
    let db = setup_db().await;

    let wasm = accept_wasm();

    // Create function + same-named bound validator.
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("shared_name", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // Drop while bound → refused.
    let mut b = Batch::new();
    b.id("df1");
    b.drop_function("op", ddl::drop_function("shared_name"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(err.code(), Some("drop_refused_bound"));

    // Unbind the validator.
    let mut b = Batch::new();
    b.id("uv");
    b.unbind_validator(
        "op",
        ddl::unbind_validator("shared_name", "users").db("testdb"),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // Now the function drop must succeed with existed: true.
    let mut b = Batch::new();
    b.id("df2");
    b.drop_function("op", ddl::drop_function("shared_name"));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(true),
        "drop should succeed with existed:true after unbind"
    );

    // Function is gone.
    let functions = db.list_functions().await.unwrap();
    assert!(
        !functions.contains(&"shared_name".to_string()),
        "function must be removed after successful drop"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// After dropping the validator entirely, the function drop succeeds.
// This covers the alternative cleanup path (drop_validator instead of unbind).
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_succeeds_after_drop_validator() {
    let db = setup_db().await;

    let wasm = accept_wasm();

    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("shared_name", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // Unbind first (drop_validator itself refuses while bound).
    let mut b = Batch::new();
    b.id("uv");
    b.unbind_validator(
        "op",
        ddl::unbind_validator("shared_name", "users").db("testdb"),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // Drop the validator entirely.
    let mut b = Batch::new();
    b.id("dv");
    b.drop_validator("op", ddl::drop_validator("shared_name"));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(true)
    );

    // Now the function drop must succeed.
    let mut b = Batch::new();
    b.id("df");
    b.drop_function("op", ddl::drop_function("shared_name"));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(true),
        "function drop should succeed after validator is dropped"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// if_exists does NOT bypass the guard: a bound same-named validator still
// causes refusal even when if_exists is set.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_if_exists_does_not_bypass_guard() {
    let db = setup_db().await;

    let wasm = accept_wasm();

    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("shared_name").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("shared_name", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // drop_function with if_exists — the function EXISTS and is bound, so the
    // guard must still refuse (if_exists only short-circuits non-existence).
    let mut b = Batch::new();
    b.id("df");
    b.drop_function("op", ddl::drop_function("shared_name").if_exists());
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("drop_refused_bound"),
        "if_exists must not bypass the bound-validator guard"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Function with NO same-named bound validator drops normally (no false
// positive from the guard).
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_unbound_succeeds() {
    let db = setup_db().await;

    let wasm = accept_wasm();

    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("lonely_fn").wasm(wasm_b64(&wasm)),
    );
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    // No validator named "lonely_fn" exists → guard is a no-op.
    let mut b = Batch::new();
    b.id("df");
    b.drop_function("op", ddl::drop_function("lonely_fn"));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(true)
    );
}
