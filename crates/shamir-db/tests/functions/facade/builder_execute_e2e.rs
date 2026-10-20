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

use std::sync::Arc;

use async_trait::async_trait;

use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
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

        let req: BatchRequest = b.to_request_via_msgpack();
        let bytes = rmp_serde::to_vec_named(&req)
            .map_err(|e| FunctionError::Compute(format!("encode BatchRequest: {e}")))?;

        // Call through the gateway — this exercises FacadeDbGateway::execute.
        let gw = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".to_string()))?;
        let resp_bytes = gw.execute(&bytes).await.map_err(FunctionError::Compute)?;

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
    shamir.create_db("testdb").await;
    shamir
}

/// Register a native function with a catalogue entry.
async fn register_native_fn(shamir: &ShamirDb, name: &str, f: Arc<dyn ShamirFunction>) {
    let empty_wasm = wat::parse_str("(module)").unwrap();
    shamir
        .create_function_from_wasm_as(name, &empty_wasm, false, Actor::System)
        .await
        .unwrap();
    shamir.functions().replace(name, f);
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
        .execute("testdb", &setup.to_request_via_msgpack())
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
        .execute("testdb", &seed.to_request_via_msgpack())
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
        .execute("testdb", &b.to_request_via_msgpack())
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
