//! Shared fixtures for the two-tier engine tests (`tests::wasm::engine`).
//!
//! The guests are copied from `benches/wasm_procedures/kernels.rs` (a test
//! cannot import a bench target): the identity echo module and the `int_sum`
//! loop kernel, both honouring the procedure ABI (`memory`, `shamir_alloc`,
//! `shamir_call`). The engine helper turns tiers on with a small `hot_calls`
//! so the background upgrade happens inside a test, and the wait helpers
//! bound every wait — never an unbounded poll.

use crate::{
    FnBatch, FnCtx, Params, ShamirFunction, Tier, TierOptions, WasmEngine, WasmEngineOptions,
    WasmFunction,
};
use shamir_types::types::value::QueryValue;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};

/// The upgrade threshold of every two-tier test: the upgrade fires on the
/// third call, well inside a test's runtime.
pub(crate) const HOT_CALLS: u64 = 3;

/// Engine with tiers on and explicit `hot_calls` / `background_jobs`.
pub(crate) fn engine_with(hot_calls: u64, background_jobs: usize) -> Arc<WasmEngine> {
    Arc::new(
        WasmEngine::with_options(WasmEngineOptions {
            tier: TierOptions {
                enabled: true,
                hot_calls,
                background_jobs,
            },
            ..Default::default()
        })
        .expect("tiered engine must build"),
    )
}

/// The default test engine: tiers on, `hot_calls = 3`, two upgrade permits.
pub(crate) fn auto_engine() -> Arc<WasmEngine> {
    engine_with(HOT_CALLS, 2)
}

// ── Guests copied from benches/wasm_procedures/kernels.rs ────────────

/// Address the guests write their one-byte result to.
const RESULT_ADDR: u32 = 2048;

/// Packs `(RESULT_ADDR, 1)` as the `shamir_call` return value.
const RETURN_RESULT_BYTE: &str = "(i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 1))";

/// Stores `acc & 127` at [`RESULT_ADDR`] and returns it.
fn finish_with_acc() -> String {
    format!(
        "(i32.store8 (i32.const {RESULT_ADDR}) (i32.and (local.get $acc) (i32.const 127)))\n    {RETURN_RESULT_BYTE}"
    )
}

/// Wraps imports, helper items, locals and the `shamir_call` body into a procedure module.
fn module(imports: &str, helpers: &str, locals: &str, body: &str, pages: u32) -> String {
    format!(
        r#"(module
  {imports}
  (memory (export "memory") {pages})
  (global $bump (mut i32) (i32.const 4096))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  {helpers}
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    {locals}
    {body}))"#
    )
}

/// Echoes the parameter region back: the cheapest possible procedure.
pub(crate) fn identity_wat() -> String {
    module(
        "",
        "",
        "",
        "(i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))",
        2,
    )
}

const INT_SUM_N: u32 = 100_000;

/// The `int_sum` loop kernel guest (copied from the bench kernels).
pub(crate) fn int_sum_wat() -> String {
    module(
        "",
        "",
        "(local $i i32) (local $acc i32)",
        &format!(
            r#"(local.set $acc (i32.const 1))
    (block $done (loop $l
      (br_if $done (i32.ge_u (local.get $i) (i32.const {INT_SUM_N})))
      (local.set $acc (i32.add
        (i32.mul (local.get $acc) (i32.const 31))
        (i32.xor (local.get $i) (i32.shr_u (local.get $acc) (i32.const 3)))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l)))
    {}"#,
            finish_with_acc()
        ),
        2,
    )
}

/// The native twin of [`int_sum_wat`]: the guest stores `acc & 127`.
pub(crate) fn int_sum_native() -> u32 {
    let mut acc: u32 = 1;
    for i in 0..black_box(INT_SUM_N) {
        acc = acc.wrapping_mul(31).wrapping_add(i ^ (acc >> 3));
    }
    black_box(acc)
}

// ── Call and wait helpers ────────────────────────────────────────────

/// One plain call through the [`ShamirFunction`] trait; a failed call is a
/// test bug, not a subject under test.
pub(crate) async fn call_once(function: &WasmFunction, params: &Params) -> QueryValue {
    function
        .call(&FnCtx::new(), &FnBatch::new(), params)
        .await
        .expect("guest call must succeed")
}

/// Identity-echo params: `{"x": 42}`.
pub(crate) fn echo_params() -> Params {
    let mut params = Params::new();
    params.set("x", QueryValue::Int(42));
    params
}

/// The expected identity-echo result: the params map round-tripped.
pub(crate) fn echo_result(params: &Params) -> QueryValue {
    QueryValue::Map(params.raw().clone())
}

/// Await `function.tier() == tier` with a hard ceiling — never an unbounded wait.
pub(crate) async fn wait_for_tier(function: &WasmFunction, tier: Tier) {
    wait_until("the tier upgrade did not complete in time", || {
        function.tier() == tier
    })
    .await;
}

/// Await `cond()` with a hard 15 s ceiling; the panic names `what`.
pub(crate) async fn wait_until(what: &'static str, cond: impl Fn() -> bool) {
    timeout(Duration::from_secs(15), async {
        while !cond() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect(what);
}
