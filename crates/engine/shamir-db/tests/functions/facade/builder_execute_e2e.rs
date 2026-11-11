//! End-to-end proof of **`DbGateway::execute`** — the general form of
//! db_get/db_insert/db_query.
//!
//! A native function builds a `BatchRequest` via the query-builder, encodes
//! it to msgpack, calls `ctx.db_gateway().execute(&bytes)`, decodes the
//! `BatchResponse`, and returns the row count. This proves the full path:
//!
//!   native fn  -->  ctx.db_gateway().execute()
//!              -->  FacadeDbGateway::execute
//!              -->  rmp_serde::from_slice (decode BatchRequest)
//!              -->  ShamirDb::execute_as (real engine, real actor)
//!              -->  rmp_serde::to_vec_named (encode BatchResponse)
//!              -->  back to native fn (decode, extract, return)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use shamir_db::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::val::lit;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_query_types::filter::FilterValue;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

// ============================================================================
// Native test procedure: RunBatchProc
// ============================================================================

/// Builds a `BatchRequest` (read `items` where `n >= 2`), encodes to msgpack,
/// calls `ctx.db_gateway().execute()`, decodes the `BatchResponse`, and
/// returns the number of matching rows as `QueryValue::Int`.
struct RunBatchProc;

#[async_trait]
impl ShamirFunction for RunBatchProc {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        // Build a BatchRequest via the query-builder: read "items" where n >= 2.
        let mut b = Batch::new();
        b.id("inner");
        b.query("rows", Query::from("items").where_gte("n", 2_i64));

        let req: BatchRequest = b.to_request_via_msgpack().expect("batch request encoding");
        let bytes = rmp_serde::to_vec_named(&req)
            .map_err(|e| FunctionError::Compute(format!("encode BatchRequest: {e}")))?;

        // Call through the gateway — this exercises FacadeDbGateway::execute.
        let gw = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".to_string()))?;
        let resp_bytes = gw
            .execute(&bytes)
            .await
            .map_err(|e| FunctionError::Compute(e.to_string()))?;

        // Decode the response.
        let resp: BatchResponse = rmp_serde::from_slice(&resp_bytes)
            .map_err(|e| FunctionError::Compute(format!("decode BatchResponse: {e}")))?;

        // Extract the row count from the "rows" result.
        let row_count = resp
            .results
            .get("rows")
            .map(|qr| qr.records.len() as i64)
            .unwrap_or(0);

        Ok(QueryValue::Int(row_count))
    }
}

// ============================================================================
// Helpers
// ============================================================================

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.expect("create_db");
    shamir
}

/// Register a native function with a catalogue entry.
async fn register_native_fn(shamir: &ShamirDb, name: &str, f: Arc<dyn ShamirFunction>) {
    let empty_wasm = wat::parse_str(
        r#"
        (module
            (memory (export "memory") 1)
            (func (export "shamir_alloc") (param i32) (result i32) i32.const 0)
            (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0))
        "#,
    )
    .unwrap();
    shamir
        .create_function_from_wasm_as(name, &empty_wasm, false, Actor::System)
        .await
        .unwrap();
    shamir.functions().replace(name, f);
}

const FUNCTION_GATEWAY_CALL_REFUSAL: &str =
    "execute: Call operations are not supported through a function database gateway";

#[derive(Clone, Copy)]
enum GatewayRequestShape {
    Direct,
    ForEach,
    WriteThenNestedBatch,
    TransactionalDirect,
    TransactionalCrossRepo,
}

struct GatewayExecuteProc {
    target: String,
    shape: GatewayRequestShape,
    invocations: Arc<AtomicUsize>,
    stop_after: Option<usize>,
}

#[async_trait]
impl ShamirFunction for GatewayExecuteProc {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let invocation = self.invocations.fetch_add(1, Ordering::Relaxed) + 1;
        if self.stop_after.is_some_and(|limit| invocation >= limit) {
            return Ok(QueryValue::Null);
        }

        let mut request = Batch::new();
        match self.shape {
            GatewayRequestShape::Direct => {
                request.call("call", self.target.as_str(), [] as [FilterValue; 0]);
            }
            GatewayRequestShape::ForEach => {
                let mut inner = Batch::new();
                inner.call("call", self.target.as_str(), [] as [FilterValue; 0]);
                request.for_each("loop", vec![lit(1_i64)], "row", inner.build());
            }
            GatewayRequestShape::WriteThenNestedBatch => {
                let write = request.insert("write", insert("items").rows([doc! { "n" => 1 }]));
                let mut nested = Batch::new();
                nested.call("call", self.target.as_str(), [] as [FilterValue; 0]);
                let nested_handle = request.sub_batch_no_bind("nested", nested.build());
                request.after(&nested_handle, &write);
            }
            GatewayRequestShape::TransactionalDirect => {
                request.transactional();
                request.call("call", self.target.as_str(), [] as [FilterValue; 0]);
            }
            GatewayRequestShape::TransactionalCrossRepo => {
                request.transactional();
                request.insert("main_write", insert("items").rows([doc! { "n" => 901 }]));
                request.insert(
                    "other_repo_write",
                    shamir_query_builder::write::Insert::with_repo("hot", "items")
                        .rows([doc! { "n" => 902 }]),
                );
            }
        }

        let req: BatchRequest = request
            .to_request_via_msgpack()
            .map_err(|e| FunctionError::Compute(format!("encode BatchRequest: {e}")))?;
        let bytes = rmp_serde::to_vec_named(&req)
            .map_err(|e| FunctionError::Compute(format!("encode BatchRequest: {e}")))?;
        let gateway = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".to_string()))?;
        gateway
            .execute(&bytes)
            .await
            .map_err(|e| FunctionError::Compute(e.to_string()))?;
        Ok(QueryValue::Null)
    }
}

struct CountingTargetProc(Arc<AtomicUsize>);

#[async_trait]
impl ShamirFunction for CountingTargetProc {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(QueryValue::Null)
    }
}

async fn setup_shamir_with_items() -> ShamirDb {
    let shamir = setup_shamir().await;
    let mut setup = Batch::new();
    setup.id("setup_items");
    setup.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["items"]),
    );
    shamir
        .execute(
            "testdb",
            &setup
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .expect("create items table");
    shamir
}

async fn setup_shamir_with_hot_items() -> ShamirDb {
    let shamir = setup_shamir_with_items().await;
    let mut setup = Batch::new();
    setup.id("setup_hot_items");
    setup.create_repo(
        "repo",
        ddl::create_repo("hot")
            .engine("in_memory")
            .tables(["items"]),
    );
    shamir
        .execute(
            "testdb",
            &setup
                .to_request_via_msgpack()
                .expect("hot repository request"),
        )
        .await
        .expect("create hot repository and items table");
    shamir
}

async fn invoke_native(shamir: &ShamirDb, name: &str) -> Result<BatchResponse, BatchError> {
    let mut request = Batch::new();
    request.id("invoke_native");
    request.call(name, name, [] as [FilterValue; 0]);
    shamir
        .execute(
            "testdb",
            &request
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
}

fn assert_function_gateway_call_refused(error: impl std::fmt::Display) {
    let message = error.to_string();
    assert!(
        message.contains(FUNCTION_GATEWAY_CALL_REFUSAL),
        "expected function gateway Call refusal, got: {message}"
    );
}

/// Bounds old gateway recursion at three depth limits; refusal must occur on the first call.
#[tokio::test]
async fn db_gateway_execute_refuses_bounded_self_call_recursion() {
    let shamir = setup_shamir().await;
    let invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_recurse",
        Arc::new(GatewayExecuteProc {
            target: "gateway_recurse".to_string(),
            shape: GatewayRequestShape::Direct,
            invocations: invocations.clone(),
            stop_after: Some(
                3 * usize::try_from(FnCtx::DEFAULT_DEPTH_LIMIT)
                    .expect("depth limit must fit in usize"),
            ),
        }),
    )
    .await;

    let error = invoke_native(&shamir, "gateway_recurse")
        .await
        .expect_err("a function gateway must refuse a nested self-call");
    assert_function_gateway_call_refused(error);
    assert_eq!(invocations.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn db_gateway_execute_refuses_direct_call_without_invoking_target() {
    let shamir = setup_shamir().await;
    let target_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_target",
        Arc::new(CountingTargetProc(target_invocations.clone())),
    )
    .await;
    let caller_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_direct_caller",
        Arc::new(GatewayExecuteProc {
            target: "gateway_target".to_string(),
            shape: GatewayRequestShape::Direct,
            invocations: caller_invocations.clone(),
            stop_after: None,
        }),
    )
    .await;

    let error = invoke_native(&shamir, "gateway_direct_caller")
        .await
        .expect_err("a function gateway must refuse a direct Call");
    assert_function_gateway_call_refused(error);
    assert_eq!(caller_invocations.load(Ordering::Relaxed), 1);
    assert_eq!(target_invocations.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn db_gateway_execute_refuses_call_buried_in_batch_before_write_dispatch() {
    let shamir = setup_shamir_with_items().await;
    let target_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_batch_target",
        Arc::new(CountingTargetProc(target_invocations.clone())),
    )
    .await;
    let caller_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_batch_caller",
        Arc::new(GatewayExecuteProc {
            target: "gateway_batch_target".to_string(),
            shape: GatewayRequestShape::WriteThenNestedBatch,
            invocations: caller_invocations.clone(),
            stop_after: None,
        }),
    )
    .await;

    let error = invoke_native(&shamir, "gateway_batch_caller")
        .await
        .expect_err("a buried Call must be refused before batch dispatch");
    assert_function_gateway_call_refused(error);
    assert_eq!(caller_invocations.load(Ordering::Relaxed), 1);
    assert_eq!(target_invocations.load(Ordering::Relaxed), 0);

    let mut read = Batch::new();
    read.id("verify_no_write");
    read.query("rows", Query::from("items"));
    let response = shamir
        .execute(
            "testdb",
            &read
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .expect("read items after refusal");
    assert!(
        response.results["rows"].records.is_empty(),
        "the write before the buried Call must not be dispatched"
    );
}

#[tokio::test]
async fn db_gateway_execute_refuses_call_buried_in_for_each() {
    let shamir = setup_shamir().await;
    let target_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_foreach_target",
        Arc::new(CountingTargetProc(target_invocations.clone())),
    )
    .await;
    let caller_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_foreach_caller",
        Arc::new(GatewayExecuteProc {
            target: "gateway_foreach_target".to_string(),
            shape: GatewayRequestShape::ForEach,
            invocations: caller_invocations.clone(),
            stop_after: None,
        }),
    )
    .await;

    let error = invoke_native(&shamir, "gateway_foreach_caller")
        .await
        .expect_err("a Call inside ForEach must be refused");
    assert_function_gateway_call_refused(error);
    assert_eq!(caller_invocations.load(Ordering::Relaxed), 1);
    assert_eq!(target_invocations.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn db_gateway_execute_refuses_call_in_transactional_request() {
    let shamir = setup_shamir().await;
    let target_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_tx_target",
        Arc::new(CountingTargetProc(target_invocations.clone())),
    )
    .await;
    let caller_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_tx_caller",
        Arc::new(GatewayExecuteProc {
            target: "gateway_tx_target".to_string(),
            shape: GatewayRequestShape::TransactionalDirect,
            invocations: caller_invocations.clone(),
            stop_after: None,
        }),
    )
    .await;

    let error = invoke_native(&shamir, "gateway_tx_caller")
        .await
        .expect_err("a transactional function gateway request must refuse Call");
    assert_function_gateway_call_refused(error);
    assert_eq!(caller_invocations.load(Ordering::Relaxed), 1);
    assert_eq!(target_invocations.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn db_gateway_execute_refuses_transactional_cross_repo_before_any_write() {
    let shamir = setup_shamir_with_hot_items().await;
    let caller_invocations = Arc::new(AtomicUsize::new(0));
    register_native_fn(
        &shamir,
        "gateway_cross_repo_caller",
        Arc::new(GatewayExecuteProc {
            target: String::new(),
            shape: GatewayRequestShape::TransactionalCrossRepo,
            invocations: caller_invocations.clone(),
            stop_after: None,
        }),
    )
    .await;

    let error = invoke_native(&shamir, "gateway_cross_repo_caller")
        .await
        .expect_err("transactional gateway requests must refuse cross-repository writes");
    assert!(
        error.to_string().contains(
            "transactional batch targets multiple repositories (hot, main); single-repo only"
        ),
        "expected transactional cross-repository refusal, got: {error}"
    );
    assert_eq!(caller_invocations.load(Ordering::Relaxed), 1);

    let mut main_read = Batch::new();
    main_read.id("verify_main_no_write");
    main_read.query("rows", Query::from("items"));
    let main_response = shamir
        .execute(
            "testdb",
            &main_read
                .to_request_via_msgpack()
                .expect("main repository verification request"),
        )
        .await
        .expect("read main repository after refusal");
    assert!(
        main_response.results["rows"].records.is_empty(),
        "main-repository insert must not be dispatched"
    );

    let mut hot_read = Batch::new();
    hot_read.id("verify_hot_no_write");
    hot_read.query("rows", Query::with_repo("hot", "items"));
    let hot_response = shamir
        .execute(
            "testdb",
            &hot_read
                .to_request_via_msgpack()
                .expect("hot repository verification request"),
        )
        .await
        .expect("read hot repository after refusal");
    assert!(
        hot_response.results["rows"].records.is_empty(),
        "hot-repository insert must not be dispatched"
    );
}

// ============================================================================
// Test
// ============================================================================

/// Proves `ctx.db_gateway().execute()` runs a full `BatchRequest` through the
/// real engine and returns a valid `BatchResponse`. The native function builds
/// a query for `items` where `n >= 2`, which should match 2 out of 3 rows.
#[tokio::test]
async fn db_gateway_execute_returns_correct_batch_response() {
    let shamir = setup_shamir().await;

    // Create repo + table.
    let mut setup = Batch::new();
    setup.id("setup");
    setup.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["items"]),
    );
    shamir
        .execute(
            "testdb",
            &setup
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .unwrap();

    // Insert 3 rows with n = 1, 2, 3.
    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "ins",
        insert("items").rows([doc! { "n" => 1 }, doc! { "n" => 2 }, doc! { "n" => 3 }]),
    );
    shamir
        .execute(
            "testdb",
            &seed
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .unwrap();

    // Register the native function that exercises db_gateway().execute().
    register_native_fn(&shamir, "run_batch", Arc::new(RunBatchProc)).await;

    // Invoke it via batch Call.
    let mut b = Batch::new();
    b.id("call_execute");
    b.call(
        "result",
        "run_batch",
        [] as [shamir_query_types::filter::FilterValue; 0],
    );
    let resp = shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();

    let qr = &resp.results["result"];
    let value = qr.value.as_ref().expect("call result must have a value");
    assert_eq!(
        value,
        &QueryValue::Int(2),
        "RunBatchProc should find 2 rows where n >= 2 (rows n=2, n=3)"
    );
}
