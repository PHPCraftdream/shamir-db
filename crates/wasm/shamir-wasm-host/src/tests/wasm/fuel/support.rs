//! Shared fixtures for the fuel-lease regression tests (`tests::wasm::fuel`).
//!
//! Guests are hand-written WAT with an explicit, deterministic busy-loop so
//! every Store segment's fuel consumption is exactly measurable via the
//! `cfg(test)` [`FuelObserver`] instrumentation (real `set_fuel` installs,
//! real suspend/settle refunds, real `get_fuel` at gate boundaries) — no
//! global counters shared across parallel tests: the observer is created
//! per invocation and threaded parent→child like the fuel pool itself.
//!
//! WAT notes (type-checked by reasoning, executed only in CI):
//! * every fixture declares exactly ONE exported `"memory"`;
//! * every fixture exports the full guest ABI (`memory`, `shamir_alloc`,
//!   `shamir_call`) so the pre-instantiation sanitizer accepts it;
//! * `i64.shl` shift counts are `i64.const` (not `i32`);
//! * the busy-loop helper uses sibling block/loop labels that are reused
//!   sequentially — legal WAT, each label's scope closes with its block.

use crate::{
    FnBatch, FnCtx, FnResult, FunctionError, FunctionRegistry, Params, WasmEngine, WasmFunction,
    WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub(crate) type Budget = Arc<AtomicU64>;

/// Deterministic busy-loop snippet burning (approximately) `n` iterations;
/// requires a local `$i i32` in the enclosing func. Labels `$done`/`$burn`
/// are reused across sibling injections — each scope closes with its block.
pub(crate) fn burn(n: i32) -> String {
    format!(
        r#"(local.set $i (i32.const 0))
    (block $done
      (loop $burn
        (br_if $done (i32.ge_u (local.get $i) (i32.const {n})))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $burn)
      )
    )"#
    )
}

/// Memory + bump-allocator preamble shared by every fixture module
/// (exactly one exported memory per module). Also declares the
/// fuel-boundary MARKER function (RV-WASM-HOST-6F709AC2 exactness): a
/// pass-through whose CALL is the guest's last act — wasmtime checks fuel
/// only at function entries and loop back-edges, so ending every guest in
/// a marker call guarantees a final check point AFTER all burn work. The
/// only charged-but-unchecked tail left is the marker's single
/// `local.get` — SOURCE-REASONED from the wasmtime 48.0.5 fuel_check
/// semantics, this is expected to make an exactly-T grant succeed while
/// T − 1 is caught by a real check (consumed >= grant) instead of by
/// unchecked-tail luck; it is NOT executed locally (policy) — CI pending.
const HEADER: &str = r#"
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func $fuel_marker (param $v i64) (result i64)
    (local.get $v))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )"#;

/// Identity echo tail: packs the original `(ptr, len)` so the msgpack
/// params round-trip as the call result, routed through the marker
/// function so a final function-entry fuel check covers ALL charged work
/// (RV-WASM-HOST-6F709AC2 exactness; see `HEADER`). `i64.shl` shifts by
/// `i64.const`.
const ECHO: &str = r#"(call $fuel_marker (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len))))"#;

/// A pure-CPU guest that loops forever (epoch-interruption fixture). Full
/// ABI exports; single memory.
pub(crate) const INFINITE_LOOP_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (loop (br 0))
    (unreachable)
  )
)"#;

/// A module whose `start` section burns fuel during instantiation — used to
/// prove instantiation-phase traps classify as typed `FuelExhausted`. The
/// full ABI (`memory`/`shamir_alloc`/`shamir_call`) is exported so the
/// sanitizer and the export lookups accept the module; `shamir_call` is a
/// plain echo that never runs (the start function traps first).
pub(crate) fn start_trap_wat() -> String {
    format!(
        r#"(module
{HEADER}
  (func $burn (local $i i32)
{BURN}
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))
  )
  (start $burn)
)"#,
        HEADER = HEADER,
        BURN = burn(5_000_000)
    )
}

/// Echo child: burns a fixed chunk, echoes the params back. No host
/// imports — the independent per-invocation cost K of every scenario.
pub(crate) fn echo_child_wat(burn_n: i32) -> String {
    format!(
        r#"(module
{HEADER}
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
{BURN}
    {ECHO}
  )
)"#,
        HEADER = HEADER,
        BURN = burn(burn_n),
        ECHO = ECHO
    )
}

/// Relay parent: burns, then — only when the `d` counter in the params is
/// still positive — copies the params to scratch, decrements the counter
/// and calls `callee`, then burns AGAIN and echoes. Work before AND after
/// the call; chainable (parent, middle, child) by naming different callees.
pub(crate) fn relay_wat(pre: i32, post: i32, callee: &str) -> String {
    format!(
        r#"(module
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (func $fuel_marker (param $v i64) (result i64)
    (local.get $v))
  (data (i32.const 0) "{callee}")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
    (local $n i32)
    ;; pre-call work
{PRE}
    ;; relay call only while the params counter is positive
    (local.set $n
      (i32.load8_u (i32.add (local.get $ptr) (i32.sub (local.get $len) (i32.const 1)))))
    (if (i32.gt_s (local.get $n) (i32.const 0))
      (then
        (memory.copy (i32.const 2048) (local.get $ptr) (local.get $len))
        (i32.store8
          (i32.add (i32.const 2048) (i32.sub (local.get $len) (i32.const 1)))
          (i32.sub (local.get $n) (i32.const 1)))
        (drop (call $host_call
          (i32.const 0) (i32.const {CALLEE_LEN})
          (i32.const 2048) (local.get $len)))))
    ;; post-call work
{POST}
    {ECHO}
  )
)"#,
        PRE = burn(pre),
        POST = burn(post),
        ECHO = ECHO,
        CALLEE_LEN = callee.len(),
        callee = callee,
    )
}

/// Scenario C parent: burns, makes THREE sequential child calls with
/// substantial burns between them, burns once more, echoes. `$i` carries
/// across the four sibling loops (3000/6000/9000/12000 totals).
pub(crate) const THREE_CALL_PARENT_WAT: &str = r#"
(module
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (func $fuel_marker (param $v i64) (result i64)
    (local.get $v))
  (data (i32.const 0) "child")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
    (local.set $i (i32.const 0))
    ;; copy params to scratch once; the child receives them verbatim
    (memory.copy (i32.const 2048) (local.get $ptr) (local.get $len))
    ;; pre-work
    (block $d0 (loop $b0
      (br_if $d0 (i32.ge_u (local.get $i) (i32.const 3000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b0)))
    (drop (call $host_call (i32.const 0) (i32.const 5) (i32.const 2048) (local.get $len)))
    (block $d1 (loop $b1
      (br_if $d1 (i32.ge_u (local.get $i) (i32.const 6000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b1)))
    (drop (call $host_call (i32.const 0) (i32.const 5) (i32.const 2048) (local.get $len)))
    (block $d2 (loop $b2
      (br_if $d2 (i32.ge_u (local.get $i) (i32.const 9000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b2)))
    (drop (call $host_call (i32.const 0) (i32.const 5) (i32.const 2048) (local.get $len)))
    ;; post-work
    (block $d3 (loop $b3
      (br_if $d3 (i32.ge_u (local.get $i) (i32.const 12000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b3)))
    ;; marker call: a final function-entry fuel check after ALL burn work
    ;; (RV-WASM-HOST-6F709AC2 exactness; see support::HEADER)
    (call $fuel_marker
      (i64.or
        (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
        (i64.extend_i32_u (local.get $len))))
  )
)"#;

/// Scenario D child: burns, then issues the gated `db_get` host import
/// (the test's gateway signals and pends forever on it).
pub(crate) const GATED_DB_CHILD_WAT: &str = r#"
(module
  (import "shamir_host" "db_get" (func $db_get (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 0) "t")
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
    (local.set $i (i32.const 0))
    (block $d (loop $b
      (br_if $d (i32.ge_u (local.get $i) (i32.const 2000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b)))
    (drop (call $db_get (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 1)))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))
  )
)"#;

/// Deterministic-epoch child: burns, calls the gated `db_get` (the test's
/// gateway kicks the engine epoch and returns immediately), then enters
/// ANOTHER busy loop — the loop back-edge is an epoch checkpoint, so the
/// already-exceeded 1-tick deadline traps `Interrupt` on the resumed guest
/// deterministically (no sleeps, no ticker-timing dependency for the trap
/// itself).
pub(crate) const GATED_DB_THEN_BURN_WAT: &str = r#"
(module
  (import "shamir_host" "db_get" (func $db_get (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 0) "t")
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
    (local.set $i (i32.const 0))
    (block $d (loop $b
      (br_if $d (i32.ge_u (local.get $i) (i32.const 2000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b)))
    (drop (call $db_get (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 1)))
    ;; post-gate loop: an epoch checkpoint the kicked epoch already exceeds
    (block $d2 (loop $b2
      (br_if $d2 (i32.ge_u (local.get $i) (i32.const 4000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b2)))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))
  )
)"#;

/// Scenario D parent: burns, then calls `child` (the gated-db child) with
/// the static empty-msgpack-map params at offset 16.
pub(crate) const CALL_GATED_CHILD_WAT: &str = r#"
(module
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 0) "child")
  ;; static empty-msgpack-map params at offset 16 (consumed by the child)
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
    (local.set $i (i32.const 0))
    (block $d (loop $b
      (br_if $d (i32.ge_u (local.get $i) (i32.const 1000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b)))
    (drop (call $host_call (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 1)))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))
  )
)"#;

/// Paused-clock deadline fixture (deterministic root-timeout test): burns,
/// then issues the gated `db_get` (stage 1 — the test's gateway pends until
/// released), then calls `child` (the gated-db child, whose own `db_get` is
/// stage 2). Two sequential gated imports across TWO Stores; the ROOT
/// depth-0 wall-clock `timeout` spans both.
pub(crate) const DB_GATE_THEN_CALL_CHILD_WAT: &str = r#"
(module
  (import "shamir_host" "db_get" (func $db_get (param i32 i32 i32 i32) (result i64)))
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 0) "t")
  (data (i32.const 16) "\80")
  (data (i32.const 32) "child")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $d (loop $b
      (br_if $d (i32.ge_u (local.get $i) (i32.const 1000)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $b)))
    (drop (call $db_get (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 1)))
    (drop (call $host_call (i32.const 32) (i32.const 5) (i32.const 16) (i32.const 1)))
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))
  )
)"#;

/// Params whose msgpack encoding ends with byte `n`: `{"d": n}`.
pub(crate) fn d_params(n: i64) -> Params {
    let mut p = Params::new();
    p.set("d", QueryValue::Int(n));
    p
}

/// The expected successful result of an echo fixture: the params map
/// round-tripped.
pub(crate) fn echo_result(params: &Params) -> QueryValue {
    QueryValue::Map(params.raw().clone())
}

/// Build a `WasmFunction` from WAT with a generous per-Store fuel limit.
pub(crate) fn fn_from_wat(engine: &Arc<WasmEngine>, wat: &str) -> Arc<WasmFunction> {
    Arc::new(
        WasmFunction::from_wat(
            engine.clone(),
            wat,
            WasmLimits {
                fuel: 1_000_000_000,
                ..WasmLimits::default()
            },
        )
        .expect("fixture WAT must compile and pass the sanitizer"),
    )
}

/// Invoke `name` top-level with an explicit pool / observer.
pub(crate) async fn invoke(
    reg: &Arc<FunctionRegistry>,
    name: &str,
    budget: Option<Budget>,
    observer: Option<Arc<crate::FuelObserver>>,
    params: &Params,
) -> FnResult<QueryValue> {
    let mut ctx = FnCtx::new().with_registry(reg.clone()).with_depth_limit(64);
    if let Some(b) = budget {
        ctx = ctx.with_fuel_budget(b);
    }
    if let Some(o) = observer {
        ctx = ctx.with_fuel_observer(o);
    }
    reg.invoke(name, &ctx, &FnBatch::new(), params).await
}

/// Register `wf` under `name` alone.
pub(crate) fn registry1(name: &str, wf: Arc<WasmFunction>) -> Arc<FunctionRegistry> {
    let reg = Arc::new(FunctionRegistry::new());
    reg.register(name, wf).expect("fresh registry");
    reg
}

/// Read the free-pool counter.
pub(crate) fn pool(b: &Budget) -> u64 {
    b.load(Ordering::Relaxed)
}

/// The invocation must be fully settled, proven STRUCTURALLY (not by
/// counting pool operations, which a mutated pool could fake):
/// * every entered Store settled exactly once;
/// * every suspension/settlement observed REAL fuel exactly 0 after its
///   revoke/clear;
/// * every install's reservation equalled the actually-installed fuel;
/// * no Store still holds executable fuel (zero active leases);
/// * the pool read is stable, and conserves against the INDEPENDENT
///   per-Store segment consumption: free == budget − burned.
pub(crate) fn assert_settled(budget: u64, b: &Budget, observer: &crate::FuelObserver) {
    observer
        .check_settled()
        .unwrap_or_else(|e| panic!("settlement invariants broken: {e}"));
    assert_eq!(
        observer.stores_with_fuel(),
        0,
        "no Store may still hold executable fuel after settlement"
    );
    assert_eq!(
        pool(b),
        b.load(Ordering::Relaxed),
        "pool changed between reads — a lease settled late"
    );
    assert_eq!(
        pool(b),
        budget - observer.burned(),
        "ledger must conserve: free == budget − burned \
         (a live lease or a double refund breaks this)"
    );
}

/// Assert an error is the TYPED per-Store fuel-exhaustion variant.
pub(crate) fn assert_fuel_typed(err: FunctionError) {
    assert!(
        matches!(err, FunctionError::FuelExhausted),
        "expected typed FuelExhausted, got: {err:?}"
    );
}
