use crate::{
    FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

/// WAT module implementing the identity echo ABI.
///
/// Exports:
/// - `memory` (2 pages = 128 KiB)
/// - `shamir_alloc(len) -> ptr`  — bump allocator starting at offset 1024
/// - `shamir_call(ptr, len) -> i64` — echoes back the same `[ptr, len)` region
const IDENTITY_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

/// WAT module that loops forever — used to test fuel exhaustion.
const INFINITE_LOOP_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (loop (br 0))
    (unreachable)
  )
)
"#;

/// WAT module whose `shamir_call` grows memory by one page each call.
const MEMORY_GROW_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    ;; Grow memory by four pages (256 KiB); trap if grow fails.
    (if (i32.eq (memory.grow (i32.const 4)) (i32.const -1))
      (then (unreachable)))
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

/// WAT module that recurses into itself via the host `call` import, burning
/// fuel on every level. Registered under the name `"recur"` (must match the
/// literal 5-byte ASCII name baked in below); `ctx.call("recur", {})`
/// re-invokes the same function, growing recursion depth by 1 each time.
///
/// Layout: `shamir_alloc` is a bump allocator starting at offset 1024. The
/// module writes the 5-byte name `"recur"` and an empty msgpack map (`0x80`,
/// one byte) into guest memory once at fixed offsets (before the bump
/// pointer moves), then in `shamir_call` burns fuel with a bounded busy loop
/// before invoking `shamir_host.call` with those fixed offsets. `shamir_call`
/// itself just echoes back its own `(ptr, len)` input (identity ABI) after
/// the recursive call returns, so the top-level caller always gets a valid
/// decodable result if recursion terminates via depth-limit rather than
/// fuel/error.
const RECURSIVE_CALL_WAT: &str = r#"
(module
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))

  (memory (export "memory") 2)
  (data (i32.const 0) "recur")
  (data (i32.const 16) "\80")

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
    ;; Burn a chunk of fuel on this level before recursing, so each level
    ;; costs real instructions (this is what makes the AGGREGATE budget
    ;; observably different from a per-Store reset budget: N levels burn
    ;; N * (loop cost), which exceeds a single Store's `limits.fuel` once N
    ;; is large enough, but would NEVER exceed it under the old
    ;; reset-per-Store behavior since each Store only ever sees its own
    ;; slice of the loop).
    (local.set $i (i32.const 0))
    (block $done
      (loop $burn
        (br_if $done (i32.ge_u (local.get $i) (i32.const 2000)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $burn)
      )
    )
    ;; Recurse: call("recur", params={}) via the host import. Name at [0,5),
    ;; empty-map params at [16,17). Ignore the returned packed result (the
    ;; identity echo below is what the top-level caller decodes).
    (drop (call $host_call
      (i32.const 0) (i32.const 5)
      (i32.const 16) (i32.const 1)))
    (i64.or
      (i64.shl
        (i64.extend_i32_u (local.get $ptr))
        (i64.const 32)
      )
      (i64.extend_i32_u (local.get $len))
    )
  )
)
"#;

fn build_identity() -> Arc<WasmFunction> {
    let engine = Arc::new(WasmEngine::new().unwrap());
    Arc::new(WasmFunction::from_wat(engine, IDENTITY_WAT, WasmLimits::default()).unwrap())
}

fn build_params() -> Params {
    let mut p = Params::new();
    p.set("x", QueryValue::Int(42));
    p.set("name", QueryValue::Str("hello".into()));
    p
}

#[tokio::test]
async fn wasm_identity_roundtrips_params() {
    let wf = build_identity();
    let params = build_params();
    let expected = QueryValue::Map(params.raw().clone());

    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &params)
        .await
        .unwrap();

    assert_eq!(result, expected);
}

#[tokio::test]
async fn wasm_identity_via_registry() {
    let reg = FunctionRegistry::new();
    let wf = build_identity();
    reg.register("echo", wf).unwrap();

    let mut params = Params::new();
    params.set("val", QueryValue::Bool(true));
    let expected = QueryValue::Map(params.raw().clone());

    let result = reg
        .invoke("echo", &FnCtx::new(), &FnBatch::new(), &params)
        .await
        .unwrap();

    assert_eq!(result, expected);
}

#[tokio::test]
async fn wasm_fuel_exhaustion_traps() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let limits = WasmLimits {
        fuel: 1000,
        ..WasmLimits::default()
    };
    let wf = Arc::new(WasmFunction::from_wat(engine, INFINITE_LOOP_WAT, limits).unwrap());

    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await;

    let err = result.unwrap_err();
    assert!(
        matches!(err, FunctionError::Compute(_)),
        "expected Compute error, got: {err:?}"
    );
}

#[tokio::test]
async fn wasm_memory_limit_enforced() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    // Each call creates a fresh store. The module starts at 2 pages; the WAT
    // tries to grow by 4 pages (256 KiB) in one shot. With a limit of 3 pages
    // total (192 KiB) the grow should fail and the guest traps.
    let limits = WasmLimits {
        fuel: 1_000_000_000,
        // Allow only 3 pages (192 KiB) total — less than the 2+4=6 the guest wants.
        max_memory_bytes: 3 * 64 * 1024,
        ..WasmLimits::default()
    };
    let wf = Arc::new(WasmFunction::from_wat(engine, MEMORY_GROW_WAT, limits).unwrap());

    let mut params = Params::new();
    params.set("k", QueryValue::Int(1));
    let result = wf.call(&FnCtx::new(), &FnBatch::new(), &params).await;
    assert!(result.is_err(), "grow beyond limit should trap");
}

#[tokio::test]
async fn wasm_wall_clock_deadline_interrupts_cpu_bound_guest() {
    // Finding: WASM aggregate fuel fan-out. A pure-CPU guest with a huge fuel
    // budget (so fuel would NOT stop it in any reasonable time) must still be
    // interrupted by the wall-clock deadline / epoch interruption instead of
    // pinning the worker indefinitely.
    let engine = Arc::new(WasmEngine::new().unwrap());
    let limits = WasmLimits {
        fuel: u64::MAX, // effectively never exhausts within the test window
        wall_clock_deadline: std::time::Duration::from_millis(300),
        ..WasmLimits::default()
    };
    let wf = Arc::new(WasmFunction::from_wat(engine, INFINITE_LOOP_WAT, limits).unwrap());

    let start = std::time::Instant::now();
    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await;
    let elapsed = start.elapsed();

    assert!(
        result.is_err(),
        "cpu-bound guest must be interrupted, not run to completion"
    );
    // Must terminate well before the (default 30s) fuel/time budget — a few
    // seconds of headroom over the 300ms deadline + epoch tick slack.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "guest ran {elapsed:?}; wall-clock deadline / epoch did not interrupt it"
    );
}

#[tokio::test]
async fn wasm_aggregate_fuel_exhausted_across_nested_calls() {
    // Finding: aggregate cross-Store fuel budget (task #612). Each recursion
    // level of `RECURSIVE_CALL_WAT` burns a fixed amount of fuel (the busy
    // loop) THEN recurses via `ctx.call`, which — before this fix — creates
    // a brand-new `Store` with a FULL `limits.fuel` grant every time. Under
    // the OLD (reset-per-Store) behavior this test would NOT catch a
    // regression: with `depth_limit` generous and per-call fuel far above
    // the per-level burn cost, every individual Store call would comfortably
    // finish within its own fresh budget, and the recursion would only ever
    // stop via the (unrelated) depth-limit guard — never via fuel exhaustion.
    // After the fix, the SAME shared `Arc<AtomicI64>` counter is threaded
    // through every nested `FnCtx`/Store, so the busy-loop cost of ALL
    // levels combined is what's checked against `limits.fuel`, and the
    // aggregate budget runs out well before `depth_limit` is reached.
    let engine = Arc::new(WasmEngine::new().unwrap());
    let limits = WasmLimits {
        fuel: 10_000,
        ..WasmLimits::default()
    };
    let wf = Arc::new(WasmFunction::from_wat(engine, RECURSIVE_CALL_WAT, limits).unwrap());

    let reg = Arc::new(FunctionRegistry::new());
    reg.register("recur", wf).unwrap();

    let ctx = FnCtx::new()
        .with_registry(reg.clone())
        .with_depth_limit(1000);

    let result = reg
        .invoke("recur", &ctx, &FnBatch::new(), &Params::new())
        .await;

    // The regression proof is that this exhausts BEFORE the generous
    // depth_limit (1000) is reached — under the old reset-per-Store fuel
    // behavior every level restarts with a full `limits.fuel` grant, so
    // recursion would only stop at `depth_limit`, never at a shared fuel
    // exhaustion. Whether the error surfaces as our own
    // "aggregate fuel budget exhausted" message (top-level check) or as a
    // wasm trap propagated up from a nested Store that ran out of its
    // (smaller-than-full) fuel grant mid-instruction, either is proof the
    // AGGREGATE budget — not a fresh per-Store budget — is what stopped
    // execution.
    result.expect_err(
        "aggregate fuel budget must exhaust before depth_limit (1000) is reached — \
         if this succeeds, fuel is being reset per Store instead of shared",
    );
}

#[tokio::test]
async fn wasm_aggregate_fuel_default_single_call_still_succeeds() {
    // Companion to the exhaustion test above: a single top-level call (no
    // nested `ctx.call`) with `limits.fuel` sized generously for its own
    // work must still succeed exactly as before this fix. The aggregate
    // budget must not make ordinary, non-recursive calls fail merely
    // because a shared counter now exists — it only matters once multiple
    // Stores draw from the SAME counter.
    let wf = build_identity();
    let params = build_params();
    let expected = QueryValue::Map(params.raw().clone());

    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &params)
        .await
        .unwrap();

    assert_eq!(result, expected);
}
