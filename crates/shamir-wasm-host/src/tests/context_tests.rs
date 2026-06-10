use crate::{FnBatch, FnCtx, FunctionRegistry, GlobalVars, Params, ShamirFunction};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

use super::super::context::BatchContext;

// ── Native test functions ─────────────────────────────────────────────

/// Writes `batch.put("tmp", Int(99))` and returns `Null`.
struct Producer;

#[async_trait]
impl ShamirFunction for Producer {
    async fn call(
        &self,
        _ctx: &FnCtx,
        batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, crate::FunctionError> {
        batch.put("tmp", QueryValue::Int(99));
        Ok(QueryValue::Null)
    }
}

/// Returns `batch.get("tmp")`, falling back to `Null`.
struct Consumer;

#[async_trait]
impl ShamirFunction for Consumer {
    async fn call(
        &self,
        _ctx: &FnCtx,
        batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, crate::FunctionError> {
        Ok(batch.get("tmp").unwrap_or(QueryValue::Null))
    }
}

/// Reads `ctx.global_get("counter")` (default 0), increments, sets it
/// back, and returns the new value.
struct GlobalBump;

#[async_trait]
impl ShamirFunction for GlobalBump {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, crate::FunctionError> {
        let current = match ctx.global_get("counter") {
            Some(QueryValue::Int(n)) => n,
            _ => 0,
        };
        let next = current + 1;
        ctx.global_set("counter", QueryValue::Int(next));
        Ok(QueryValue::Int(next))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn batch_context_shares_between_functions() {
    let reg = FunctionRegistry::new();
    reg.register("producer", Arc::new(Producer)).unwrap();
    reg.register("consumer", Arc::new(Consumer)).unwrap();

    let ctx = Arc::new(BatchContext::new());
    let batch = FnBatch::with_context(ctx);
    let fctx = FnCtx::new();

    reg.invoke("producer", &fctx, &batch, &Params::new())
        .await
        .unwrap();

    let result = reg
        .invoke("consumer", &fctx, &batch, &Params::new())
        .await
        .unwrap();

    assert_eq!(result, QueryValue::Int(99));
}

#[tokio::test]
async fn batch_context_is_isolated() {
    let reg = FunctionRegistry::new();
    reg.register("producer", Arc::new(Producer)).unwrap();
    reg.register("consumer", Arc::new(Consumer)).unwrap();

    let ctx_a = Arc::new(BatchContext::new());
    let ctx_b = Arc::new(BatchContext::new());
    let batch_a = FnBatch::with_context(ctx_a);
    let batch_b = FnBatch::with_context(ctx_b);
    let fctx = FnCtx::new();

    reg.invoke("producer", &fctx, &batch_a, &Params::new())
        .await
        .unwrap();

    let result = reg
        .invoke("consumer", &fctx, &batch_b, &Params::new())
        .await
        .unwrap();

    assert_eq!(result, QueryValue::Null);
}

#[tokio::test]
async fn globals_persist_across_invocations() {
    let reg = FunctionRegistry::new();
    reg.register("bump", Arc::new(GlobalBump)).unwrap();

    let globals = Arc::new(GlobalVars::new());
    let fctx = FnCtx::with_globals(globals.clone());
    let batch = FnBatch::new();

    let r1 = reg
        .invoke("bump", &fctx, &batch, &Params::new())
        .await
        .unwrap();
    assert_eq!(r1, QueryValue::Int(1));

    let r2 = reg
        .invoke("bump", &fctx, &batch, &Params::new())
        .await
        .unwrap();
    assert_eq!(r2, QueryValue::Int(2));

    // A separate read through the globals store confirms persistence.
    assert_eq!(globals.get("counter"), Some(QueryValue::Int(2)));
}
