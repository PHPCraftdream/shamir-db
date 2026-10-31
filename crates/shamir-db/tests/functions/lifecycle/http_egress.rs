//! HTTP egress and per-function `net_grants` capability end-to-end tests.

use shamir_db::shamir_db::FunctionSource;
use shamir_db::ShamirDb;
use shamir_engine::function::{CreateFunctionOptions, Params, Security, Visibility};
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::CreateFunctionOp;
use shamir_query_types::batch::BatchOp;
use shamir_storage::error::DbError;
use shamir_types::types::value::QueryValue;

use super::support::{compile_or_skip, curl_available, start_mock_http_server};

// ── Slice 8c: HTTP egress tests ──────────────────────────────────────

/// Source for a function that tries `ctx.http_get("http://evil.example.com/")`.
/// With an empty allowlist the gateway is present but denies the request
/// (catchable Err, not a trap). The function handles the error and returns
/// a descriptive string. No network access needed.
const EGRESS_DENIED_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn try_evil(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    match ctx.http_get("http://evil.example.com/") {
        Ok(resp) => Ok(Value::Int(resp.status() as i64)),
        Err(e) => Ok(Value::Str(format!("err:{}", e.message()))),
    }
}
"#;

/// Source for a function that does `ctx.http_get(url)` and returns the
/// response body as a string. The URL is passed via params so the test can
/// inject the mock server's dynamic port.
const HTTP_FETCH_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn fetcher(ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let url = params.str("url")?;
    match ctx.http_get(url) {
        Ok(resp) => Ok(Value::Str(resp.body_text())),
        Err(e) => Ok(Value::Str(format!("err:{}", e.message()))),
    }
}
"#;

/// A function that tries http_get with an empty allowlist must receive a
/// catchable error (the guard rejects before any network I/O). The gateway
/// is present (deny-all policy), so the function can handle the Err.
/// Toolchain-gated, no curl or network needed.
#[tokio::test]
async fn egress_denied_by_default() {
    let db = ShamirDb::init_memory().await.unwrap();

    if !compile_or_skip(&db, "try_evil", EGRESS_DENIED_SRC).await {
        return;
    }

    // Invoke WITHOUT setting a net allowlist — gateway present, deny-all.
    // The function catches the Err and returns a descriptive string.
    let result = db.invoke_function("try_evil", Params::new()).await.unwrap();

    match result {
        QueryValue::Str(s) if s.starts_with("err:") => {
            assert!(
                s.contains("not allowed") || s.contains("egress"),
                "error should mention egress denial, got: {s}"
            );
        }
        other => {
            panic!(
                "expected an error string from the function (egress denied), got: {:?}",
                other
            );
        }
    }
}
/// A WASM function does `ctx.http_get(url)` against a local mock server.
/// The allowlist is set to `["127.0.0.1"]` (exact entry, required for
/// loopback). Toolchain + curl gated — skips cleanly if either is absent.
#[tokio::test]
async fn wasm_function_http_fetch_allowed() {
    if !curl_available() {
        eprintln!("SKIP wasm_function_http_fetch_allowed — curl not found on PATH");
        return;
    }

    let mock_body = b"hello from mock server 8c";
    let port = start_mock_http_server(mock_body).await;

    let mut db = ShamirDb::init_memory().await.unwrap();
    db.set_net_allowlist(vec!["127.0.0.1".to_string()]);

    // Task #609: empty `net_grants` now denies all egress, so this test
    // (which makes a REAL network call and expects it to succeed) must
    // grant the mock server's host explicitly.
    let opts = CreateFunctionOptions {
        replace: false,
        visibility: Visibility::Private,
        security: Security::Invoker,
        secret_grants: Vec::new(),
        net_grants: vec!["127.0.0.1".to_string()],
    };
    match db
        .create_function_with_opts("fetcher", FunctionSource::Source(HTTP_FETCH_SRC), opts)
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP wasm_function_http_fetch_allowed — toolchain unavailable: {msg}");
            return;
        }
        Err(e) => panic!("compile fetcher failed: {e}"),
    }

    let mut params = Params::new();
    params.set("url", QueryValue::Str(format!("http://127.0.0.1:{port}/")));

    let result = db.invoke_function("fetcher", params).await.unwrap();

    match result {
        QueryValue::Str(body) => {
            assert_eq!(
                body, "hello from mock server 8c",
                "function should return the mock server's response body"
            );
        }
        other => {
            panic!("expected string from fetcher, got: {:?}", other);
        }
    }
}

// ── #544/#609: per-function net_grants capability ─────────────────────

/// A function created with the default options (empty `net_grants`) must
/// be denied ALL egress, even to a host the DB-wide `net_allowlist`
/// permits — task #609 flips the empty-list default to match
/// `secret_grants`'s restrictive-by-default precedent: empty `net_grants`
/// means "no egress at all" for this function, not "fall back to the DB
/// allowlist". This pins the invariant explicitly via the allowlist-denial
/// error string so a future accidental flip back to the permissive
/// semantics fails loudly here even without curl/network available.
#[tokio::test]
async fn net_grants_empty_denies_all_egress() {
    let mut db = ShamirDb::init_memory().await.unwrap();
    // DB-wide allowlist permits an example host; the function has NO
    // net_grants configured (default options) — must still be denied.
    db.set_net_allowlist(vec!["allowed.example.com".to_string()]);

    if !compile_or_skip(&db, "fetcher", HTTP_FETCH_SRC).await {
        return;
    }

    let mut params = Params::new();
    params.set(
        "url",
        QueryValue::Str("http://allowed.example.com/".to_string()),
    );
    let result = db.invoke_function("fetcher", params).await.unwrap();
    match result {
        QueryValue::Str(s) if s.starts_with("err:") => {
            assert!(
                s.contains("not allowed"),
                "empty net_grants must deny ALL egress (task #609, restrictive \
                 default) — expected an allowlist-denial error, got: {s}"
            );
        }
        other => panic!(
            "expected an allowlist-denial error string for a function with \
             empty net_grants, got: {:?}",
            other
        ),
    }
}

/// Source for a function that tries `ctx.http_get(url)` against a
/// caller-supplied URL and reports whether the FAILURE was an allowlist
/// denial specifically (vs. a network-level failure), so the test can
/// distinguish "net_grants correctly scoped this down" from "network is
/// just unavailable in this sandbox".
const NET_GRANTS_FETCH_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn scoped_fetcher(ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let url = params.str("url")?;
    match ctx.http_get(url) {
        Ok(resp) => Ok(Value::Str(resp.body_text())),
        Err(e) => Ok(Value::Str(format!("err:{}", e.message()))),
    }
}
"#;

/// A function with an explicit, NARROWER `net_grants` list must be denied
/// egress to a host the DB-wide allowlist would otherwise permit but that
/// is outside the function's own grant — the actual per-function scoping
/// behavior finding 1 closes (task #544), independent of the empty-list
/// question above. `build_net_gateway` must INTERSECT `net_grants` with
/// `net_allowlist`, not just check the DB-wide list.
///
/// Toolchain-gated — skips cleanly when cargo / wasm32 target is absent.
/// No curl/network needed: the allowlist guard rejects before any I/O.
#[tokio::test]
async fn net_grants_narrower_than_db_allowlist_denies_outside_host() {
    let mut db = ShamirDb::init_memory().await.unwrap();
    // DB-wide allowlist permits BOTH hosts.
    db.set_net_allowlist(vec![
        "allowed.example.com".to_string(),
        "other.example.com".to_string(),
    ]);

    let opts = CreateFunctionOptions {
        replace: false,
        visibility: Visibility::Private,
        security: Security::Invoker,
        secret_grants: Vec::new(),
        // Function-level grant narrows it to ONLY "allowed.example.com" —
        // "other.example.com" is DB-allowed but must be denied to THIS
        // function specifically.
        net_grants: vec!["allowed.example.com".to_string()],
    };
    match db
        .create_function_with_opts(
            "scoped_fetcher",
            FunctionSource::Source(NET_GRANTS_FETCH_SRC),
            opts,
        )
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!(
                "SKIP net_grants_narrower_than_db_allowlist_denies_outside_host — toolchain unavailable: {msg}"
            );
            return;
        }
        Err(e) => panic!("compile scoped_fetcher failed: {e}"),
    }

    // Request the host that's DB-allowed but OUTSIDE this function's own
    // net_grants — must be denied.
    let mut params = Params::new();
    params.set(
        "url",
        QueryValue::Str("http://other.example.com/".to_string()),
    );
    let result = db.invoke_function("scoped_fetcher", params).await.unwrap();
    match result {
        QueryValue::Str(s) if s.starts_with("err:") => {
            assert!(
                s.contains("not allowed"),
                "host outside the function's own net_grants must be denied \
                 by the allowlist guard, got: {s}"
            );
        }
        other => panic!(
            "expected an allowlist-denial error string for a host outside \
             net_grants, got: {:?}",
            other
        ),
    }
}

/// Task #609 wire-surface check: a function created via the wire
/// `create_function` batch op (`BatchOp::CreateFunction` /
/// `handle_create_function`), with `net_grants` left at its default (empty
/// `Vec`), must get NO egress at all — even to a host the DB-wide
/// `net_allowlist` permits. This guards the wire-path plumbing added in
/// `admin_function.rs::handle_create_function` (previously hardcoded to
/// `Vec::new()` regardless of what the caller wanted; now threads
/// `op.net_grants` through), independent of the embedded-API path already
/// covered by `net_grants_empty_denies_all_egress` above.
#[tokio::test]
async fn create_function_with_empty_net_grants_via_wire_gets_no_egress() {
    let mut db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.expect("create_db");
    db.set_net_allowlist(vec!["allowed.example.com".to_string()]);

    let mut b = Batch::new();
    b.id("cf");
    b.create_function(
        "op",
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: "wire_fetcher".to_string(),
            source: Some(HTTP_FETCH_SRC.to_string()),
            wasm: None,
            replace: false,
            visibility: None,
            security: None,
            secret_grants: Vec::new(),
            net_grants: Vec::new(), // default — must deny all egress (task #609)
            hmac: None,
        }),
    );
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    match db.execute("testdb", &req).await {
        Ok(_) => {}
        Err(e) if e.to_string().contains("toolchain unavailable") => {
            eprintln!(
                "SKIP create_function_with_empty_net_grants_via_wire_gets_no_egress — toolchain unavailable: {e}"
            );
            return;
        }
        Err(e) => panic!("create_function via wire failed: {e}"),
    }

    let mut params = Params::new();
    params.set(
        "url",
        QueryValue::Str("http://allowed.example.com/".to_string()),
    );
    let result = db.invoke_function("wire_fetcher", params).await.unwrap();
    match result {
        QueryValue::Str(s) if s.starts_with("err:") => {
            assert!(
                s.contains("not allowed"),
                "function created via the wire with default (empty) net_grants \
                 must deny ALL egress (task #609), got: {s}"
            );
        }
        other => panic!(
            "expected an allowlist-denial error string for a wire-created \
             function with empty net_grants, got: {:?}",
            other
        ),
    }
}
