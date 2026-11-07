//! Function wire ops e2e: describe / export / check / alter + create
//! `lang`/`optimize` validation, over the msgpack wire executor.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{ResourceMeta, ResourcePath};
use shamir_types::types::value::QueryValue;

use crate::helpers::*;

// ── small local extraction helpers (index-based, no whole-record asserts) ──

fn expect_str<'a>(rec: &'a QueryValue, key: &str) -> &'a str {
    rec[key]
        .as_str()
        .unwrap_or_else(|| panic!("record key '{key}' is not a string: {:?}", rec[key]))
}

fn expect_int(rec: &QueryValue, key: &str) -> i64 {
    rec[key]
        .as_i64()
        .unwrap_or_else(|| panic!("record key '{key}' is not an int: {:?}", rec[key]))
}

fn expect_null(rec: &QueryValue, key: &str) {
    assert!(
        matches!(rec[key], QueryValue::Null),
        "record key '{key}' should be null, got: {:?}",
        rec[key]
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 1. create with optimize "never" → describe shows mode never, tier fast
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_optimize_never_describes_never_and_fast_tier() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_never")
            .wasm(wasm_b64(&wasm))
            .optimize("never")
            .build()
            .expect("valid create function"),
    );
    let create_req = b.to_request_via_msgpack().expect("batch request encoding");
    db.execute("testdb", &create_req).await.unwrap();

    let mut b = Batch::new();
    b.id("df");
    b.describe_function("op", ddl::describe_function("fn_never"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();

    assert_eq!(expect_str(&rec, "optimize"), "never");
    assert_eq!(expect_str(&rec["tier"], "tier"), "fast");
}

// ═══════════════════════════════════════════════════════════════════════
// 2. export returns the uploaded bytes (base64) and the describe hash
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn export_returns_uploaded_bytes_and_hash() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_export")
            .wasm(wasm_b64(&wasm))
            .build()
            .expect("valid create function"),
    );
    let create_req = b.to_request_via_msgpack().expect("batch request encoding");
    db.execute("testdb", &create_req).await.unwrap();

    // Describe first: capture the hash (never assert a hard-coded hex).
    let mut b = Batch::new();
    b.id("df");
    b.describe_function("op", ddl::describe_function("fn_export"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = db.execute("testdb", &req).await.unwrap();
    let described = resp.results["op"].records[0].as_value().into_owned();
    let described_hash = expect_str(&described, "hash").to_string();
    assert!(!described_hash.is_empty());

    let mut b = Batch::new();
    b.id("ef");
    b.export_function("op", ddl::export_function("fn_export"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();

    assert_eq!(expect_str(&rec, "wasm"), wasm_b64(&wasm));
    expect_null(&rec, "source");
    assert_eq!(expect_str(&rec, "hash"), described_hash);
}

// ═══════════════════════════════════════════════════════════════════════
// 3. check of a valid module → report; nothing persisted
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn check_valid_module_reports_and_persists_nothing() {
    let db = setup_db().await;
    let before = db.list_functions().await.unwrap();

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("ck");
    b.try_check_function("op", ddl::check_function().wasm(wasm_b64(&wasm)))
        .expect("valid check function");
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();

    assert!(matches!(rec["ok"], QueryValue::Bool(true)));
    assert!(expect_int(&rec, "size") > 0);
    assert!(matches!(rec["abi"], QueryValue::Int(_)));
    expect_int(&rec, "functions");
    expect_int(&rec, "imports");
    expect_int(&rec, "exports");

    // A check must not persist anything: the catalogue is unchanged.
    assert_eq!(db.list_functions().await.unwrap(), before);
}

// ═══════════════════════════════════════════════════════════════════════
// 4. check of a SIMD module is an error naming the feature
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn check_simd_module_names_the_rejected_feature() {
    let db = setup_db().await;

    let simd_wat = r#"
(module
  (func (export "simd_fn") (result v128) (v128.const i32x4 0 0 0 0)))
"#;
    let wasm = wat::parse_str(simd_wat).expect("WAT parse failed");
    let mut b = Batch::new();
    b.id("ck");
    b.try_check_function("op", ddl::check_function().wasm(wasm_b64(&wasm)))
        .expect("valid check function");
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db.execute("testdb", &req).await.unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("simd"),
        "admission error should name the rejected feature, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 5. alter changes the described optimize mode
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn alter_changes_described_mode() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_alter")
            .wasm(wasm_b64(&wasm))
            .build()
            .expect("valid create function"),
    );
    let create_req = b.to_request_via_msgpack().expect("batch request encoding");
    db.execute("testdb", &create_req).await.unwrap();

    let mut b = Batch::new();
    b.id("af");
    b.alter_function("op", ddl::alter_function("fn_alter", "now"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();
    assert_eq!(expect_str(&rec, "altered_function"), "fn_alter");
    assert_eq!(expect_str(&rec, "optimize"), "now");

    let mut b = Batch::new();
    b.id("df");
    b.describe_function("op", ddl::describe_function("fn_alter"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();
    assert_eq!(expect_str(&rec, "optimize"), "now");
    assert_eq!(expect_str(&rec["tier"], "mode"), "now");
}

// ═══════════════════════════════════════════════════════════════════════
// 6. create_function lang/optimize rejections with exact messages
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_lang_shamir_is_rejected() {
    let db = setup_db().await;
    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_shamir")
            .wasm(wasm_b64(&wasm))
            .lang("shamir")
            .build()
            .expect("valid create function"),
    );
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("lang \"shamir\" is not available yet"),
        "got: {err}"
    );
}

#[tokio::test]
async fn create_lang_rust_with_wasm_payload_is_rejected() {
    let db = setup_db().await;
    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_mismatch")
            .wasm(wasm_b64(&wasm))
            .lang("rust")
            .build()
            .expect("valid create function"),
    );
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert!(
        err.to_string().contains("requires the \"source\" field"),
        "got: {err}"
    );
}

#[tokio::test]
async fn create_unknown_lang_is_rejected() {
    let db = setup_db().await;
    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_unknown_lang")
            .wasm(wasm_b64(&wasm))
            .lang("python")
            .build()
            .expect("valid create function"),
    );
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert!(err.to_string().contains("unknown lang"), "got: {err}");
}

#[tokio::test]
async fn create_unknown_optimize_is_rejected() {
    let db = setup_db().await;
    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_unknown_opt")
            .wasm(wasm_b64(&wasm))
            .optimize("sometimes")
            .build()
            .expect("valid create function"),
    );
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert!(err.to_string().contains("unknown optimize"), "got: {err}");
}

// ═══════════════════════════════════════════════════════════════════════
// 7. describe / export / alter permission denials for a non-privileged actor
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn describe_export_alter_denied_for_non_privileged_actor() {
    let db = setup_db().await;

    // Open the db so traversal is NOT the blocker: the denial must come
    // from the function resource's own owner-only meta (System-owned).
    db.set_resource_meta(&ResourcePath::database("testdb"), &ResourceMeta::open())
        .await
        .unwrap();

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        ddl::create_function("fn_guarded")
            .wasm(wasm_b64(&wasm))
            .build()
            .expect("valid create function"),
    );
    let create_req = b.to_request_via_msgpack().expect("batch request encoding");
    db.execute("testdb", &create_req).await.unwrap();

    let user = shamir_types::access::Actor::User(42);

    let mut b = Batch::new();
    b.id("df");
    b.describe_function("op", ddl::describe_function("fn_guarded"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db
        .execute_as(user.clone(), "testdb", &req)
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "describe_function: {err}"
    );
    assert!(
        err.to_string().contains("access denied"),
        "describe_function must be denied, got: {err}"
    );

    let mut b = Batch::new();
    b.id("ef");
    b.export_function("op", ddl::export_function("fn_guarded"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db
        .execute_as(user.clone(), "testdb", &req)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("access_denied"), "export_function: {err}");
    assert!(
        err.to_string().contains("access denied"),
        "export_function must be denied, got: {err}"
    );

    let mut b = Batch::new();
    b.id("af");
    b.alter_function("op", ddl::alter_function("fn_guarded", "now"));
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let err = db.execute_as(user, "testdb", &req).await.unwrap_err();
    assert_eq!(err.code(), Some("access_denied"), "alter_function: {err}");
    assert!(
        err.to_string().contains("access denied"),
        "alter_function must be denied, got: {err}"
    );
}
