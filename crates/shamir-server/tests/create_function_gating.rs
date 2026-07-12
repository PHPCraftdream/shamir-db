//! Wire-level gating tests for `CreateFunctionOp`'s per-field
//! `visibility` / `security` / `secret_grants` plumbing (task #554).
//!
//! These exercise the THREE different gates the brief requires:
//!   * `visibility` — no gate (Private is the default; Public on your own
//!     new resource is harmless). Verified by threading + persisted metadata.
//!   * `security: "definer"` — CONDITIONAL HMAC gate (tag required IFF
//!     definer). missing → `hmac_required`; wrong → `hmac_mismatch`;
//!     correct → succeeds and persists `Security::Definer`.
//!   * `secret_grants` non-empty — CONDITIONAL HMAC gate (same as definer)
//!     AND an additional `Manage(Root)` authorization gate. The auth gate
//!     is exercised in-process in `shamir-db` (a non-System actor can't
//!     reach the function-creation handler over the wire — the coarse
//!     superuser gate rejects it first with `permission_denied`); here we
//!     cover the HMAC half and the persisted-metadata half with a
//!     superuser session (System bypasses `authorize_access`, so the
//!     `Manage(Root)` half is a no-op for these wire cases).
//!
//! Plus a regression test confirming that a plain `CreateFunctionOp`
//! (neither `definer` nor non-empty `secret_grants`) requires NO hmac at
//! all — the conditional gate must not accidentally make hmac mandatory
//! for ordinary function creation.

use std::sync::Arc;

use base64::Engine;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_db::engine::function::{Security, Visibility};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

/// Build a superuser session. Bypassing the SessionStore means
/// `session_id` stays at zeros, which is fine — what matters for
/// HMAC validation is that the test computes its tag with the
/// SAME `session_id` the server sees.
fn root_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

fn session_key(session: &Session) -> [u8; 32] {
    canon::derive_session_hmac_key(&session.session_id)
}

async fn make_db(db: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    Arc::new(shamir)
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

fn expect_error(res: DbResponse) -> (String, String) {
    match res {
        DbResponse::Error { code, message } => (code, message),
        other => panic!("expected Error, got {:?}", other),
    }
}

fn expect_batch_ok(res: DbResponse) -> shamir_db::query::batch::BatchResponse {
    match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch, got {:?}", other),
    }
}

/// Identity-echo WAT — a minimal valid WASM module that satisfies the
/// slice-2 ABI (exports `memory`, `shamir_alloc`, `shamir_call`). Used so
/// the create_function op can actually compile + persist without needing
/// the cargo/wasm32 toolchain.
const ECHO_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))))
"#;

/// Base64-encoded echo WASM bytes for the wire `wasm` field.
fn echo_wasm_b64() -> String {
    let wasm = wat::parse_str(ECHO_WAT).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&wasm)
}

async fn handle(handler: &ShamirDbHandler, session: &Session, req: &DbRequest) -> DbResponse {
    decode(
        &handler
            .handle(session, &encode(req), &ConnectionServices::without_push(0))
            .await
            .unwrap(),
    )
}

// --------------------------------------------------------------------------
// 1. visibility: "public" — no hmac required, succeeds, persists Public.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_visibility_public_no_hmac_succeeds() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_function(
        "f",
        ddl::create_function("vis_pub")
            .wasm(echo_wasm_b64())
            .visibility("public"),
    );
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let resp = expect_batch_ok(res);
    let rec = &resp.results["f"].records[0];
    assert_eq!(rec.get_value_str("created_function"), Some("vis_pub"));

    // Persisted metadata must reflect the requested visibility.
    let meta = shamir.function_meta("vis_pub").expect("meta present");
    assert_eq!(meta.visibility, Visibility::Public);
    // Security unchanged from default.
    assert_eq!(meta.security, Security::Invoker);
}

// --------------------------------------------------------------------------
// 2. security: "definer" WITHOUT hmac → hmac_required.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_security_definer_without_hmac_rejected() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_function(
        "f",
        ddl::create_function("sec_def_no_hmac")
            .wasm(echo_wasm_b64())
            .security("definer"),
    );
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

// --------------------------------------------------------------------------
// 3. security: "definer" WITH correct hmac → succeeds, persists Definer.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_security_definer_with_correct_hmac_succeeds() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let session = root_session();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_function("sec_def_ok", "definer", &[]),
    );
    let mut b = Batch::new();
    b.id(1);
    b.create_function(
        "f",
        ddl::create_function("sec_def_ok")
            .wasm(echo_wasm_b64())
            .security("definer")
            .hmac(&tag),
    );
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let resp = expect_batch_ok(res);
    let rec = &resp.results["f"].records[0];
    assert_eq!(rec.get_value_str("created_function"), Some("sec_def_ok"));

    let meta = shamir.function_meta("sec_def_ok").expect("meta present");
    assert_eq!(meta.security, Security::Definer);
}

// --------------------------------------------------------------------------
// 4. security: "definer" WITH wrong hmac → hmac_mismatch.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_security_definer_with_wrong_hmac_rejected() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_function(
        "f",
        ddl::create_function("sec_def_bad")
            .wasm(echo_wasm_b64())
            .security("definer")
            .hmac("deadbeef".repeat(8)), // 64 hex chars but bogus
    );
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

// --------------------------------------------------------------------------
// 5. secret_grants non-empty WITHOUT hmac → hmac_required.
//    (The Manage(Root) authorization half is covered in-process in
//    shamir-db: a superuser session maps to Actor::System which bypasses
//    authorize_access, so the auth gate is a no-op over the wire; the
//    HMAC half is what these wire tests exercise.)
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_secret_grants_without_hmac_rejected() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_function(
        "f",
        ddl::create_function("grants_no_hmac")
            .wasm(echo_wasm_b64())
            .secret_grants(["ADMIN_DB_PASSWORD"]),
    );
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

// --------------------------------------------------------------------------
// 6. secret_grants + Manage(Root) [superuser] + correct hmac → succeeds,
//    persisted secret_grants match.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_secret_grants_with_correct_hmac_succeeds() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let session = root_session();

    let grants = vec!["FOO".to_string(), "BAR".to_string()];
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_function("grants_ok", "invoker", &grants),
    );
    let mut b = Batch::new();
    b.id(1);
    b.create_function(
        "f",
        ddl::create_function("grants_ok")
            .wasm(echo_wasm_b64())
            .secret_grants(grants.clone())
            .hmac(&tag),
    );
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let resp = expect_batch_ok(res);
    let rec = &resp.results["f"].records[0];
    assert_eq!(rec.get_value_str("created_function"), Some("grants_ok"));

    let meta = shamir.function_meta("grants_ok").expect("meta present");
    assert_eq!(meta.secret_grants, grants);
}

// --------------------------------------------------------------------------
// 7. Regression: plain CreateFunctionOp (neither definer nor non-empty
//    secret_grants) requires NO hmac at all.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_plain_no_hmac_required() {
    let shamir = make_db("scratch").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_function("f", ddl::create_function("plain_fn").wasm(echo_wasm_b64()));
    let res = handle(&handler, &session, &execute_built("scratch", b.build())).await;
    let resp = expect_batch_ok(res);
    let rec = &resp.results["f"].records[0];
    assert_eq!(rec.get_value_str("created_function"), Some("plain_fn"));

    // Defaults preserved.
    let meta = shamir.function_meta("plain_fn").expect("meta present");
    assert_eq!(meta.visibility, Visibility::Private);
    assert_eq!(meta.security, Security::Invoker);
    assert!(meta.secret_grants.is_empty());
}
