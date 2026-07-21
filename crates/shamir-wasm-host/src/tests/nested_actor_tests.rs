//! RI-7 regression tests — a nested `ctx.call(...)` inherits the PARENT
//! `FnCtx`'s actor, not `Actor::System`.
//!
//! Before RI-7, `host_call.rs` rebuilt the child `FnCtx` WITHOUT threading the
//! parent's actor, so the child silently defaulted to `Actor::System`. That was
//! a confused-deputy privilege-escalation primitive: any principal able to
//! trigger a function making ONE nested call reached `Actor::System` one hop
//! in, regardless of the nested function's own actor checks.
//!
//! The fix threads `actor` through `HostState` → `host_call` Phase 1 clone →
//! child `FnCtx::with_actor`. These tests prove a nested call observes the
//! SAME actor as the parent invocation.
//!
//! Revert-and-confirm-fail proof: under the OLD behavior (`host_call` left the
//! child `FnCtx`'s actor at its `FnCtx::with_globals` default `Actor::System`),
//! [`nested_wasm_call_inherits_parent_actor`] would FAIL — the probe would see
//! `Actor::System != Actor::User(42)` and return `Int(0)`, so the assertion
//! `result == Int(1)` fails. Re-applying the fix makes it pass.

use async_trait::async_trait;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

use crate::{
    FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction, WasmEngine,
    WasmFunction, WasmLimits,
};

// ── Native probe ───────────────────────────────────────────────────────────

/// A native [`ShamirFunction`] that returns `Int(1)` iff the actor it observes
/// via `ctx.actor()` equals `expected`, else `Int(0)`.
///
/// Invoked as the NESTED call target of [`CALLER_PROBE_WAT`], so its return
/// value propagates back through the WASM call chain as the observable of
/// which actor `host_call` threaded into the child `FnCtx`.
struct ActorProbe {
    expected: Actor,
}

#[async_trait]
impl ShamirFunction for ActorProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        if ctx.actor() == &self.expected {
            Ok(QueryValue::Int(1))
        } else {
            Ok(QueryValue::Int(0))
        }
    }
}

// ── WASM caller module ─────────────────────────────────────────────────────

/// WAT module whose `shamir_call` invokes `shamir_host.call("probe", {})` and
/// returns the packed result directly. The probe's msgpack result is written
/// into THIS module's memory by `host_call` Phase 3, then read back out by
/// `WasmFunction::call` — so the probe's `Int(1)`/`Int(0)` becomes the decoded
/// top-level result, with no shared mutable state required.
///
/// Layout: `"probe"` at `[0,5)`, empty msgpack map `{}` (`0x80`) at `[16,17)`,
/// bump allocator from offset 1024.
const CALLER_PROBE_WAT: &str = r#"
(module
  (import "shamir_host" "call" (func $host_call (param i32 i32 i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 0) "probe")
  (data (i32.const 16) "\80")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  ;; Ignore the input (ptr,len); call host_call("probe", {}) and forward its
  ;; packed result. host_call writes the probe's output into our memory and
  ;; returns (ptr,len) — which WasmFunction::call then reads back.
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (call $host_call
      (i32.const 0) (i32.const 5)
      (i32.const 16) (i32.const 1))
  )
)
"#;

// ── Tests ──────────────────────────────────────────────────────────────────

/// A nested `ctx.call("probe", {})` invoked from a WASM function whose parent
/// `FnCtx` carries `Actor::User(42)` must let the probe observe `Actor::User(42)`.
///
/// Revert-and-confirm-fail proof: under the OLD behavior (`host_call` did NOT
/// call `.with_actor(...)` when building the child `FnCtx`, so the child
/// defaulted to `Actor::System`), the probe would observe `Actor::System !=
/// Actor::User(42)` and return `Int(0)` — this assertion (`result == Int(1)`)
/// would fail.
#[tokio::test]
async fn nested_wasm_call_inherits_parent_actor() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let caller =
        Arc::new(WasmFunction::from_wat(engine, CALLER_PROBE_WAT, WasmLimits::default()).unwrap());
    let probe = Arc::new(ActorProbe {
        expected: Actor::User(42),
    });

    let reg = Arc::new(FunctionRegistry::new());
    reg.register("caller", caller).unwrap();
    reg.register("probe", probe).unwrap();

    // Parent invocation carries actor = User(42).
    let ctx = FnCtx::new()
        .with_registry(reg.clone())
        .with_actor(Actor::User(42));

    let result = reg
        .invoke("caller", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap();

    assert_eq!(
        result,
        QueryValue::Int(1),
        "nested ctx.call must inherit the parent's actor (User(42)); \
         Int(0) means the probe saw Actor::System (the OLD confused-deputy bug)"
    );
}

/// Companion: when the parent carries `Actor::System` explicitly, the nested
/// probe ALSO sees `Actor::System` and returns `Int(1)` (the probe's expected
/// actor is System here). This proves the inheritance is faithful — it tracks
/// the parent's actor in BOTH directions, not a one-sided override.
#[tokio::test]
async fn nested_wasm_call_inherits_system_when_parent_is_system() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let caller =
        Arc::new(WasmFunction::from_wat(engine, CALLER_PROBE_WAT, WasmLimits::default()).unwrap());
    let probe = Arc::new(ActorProbe {
        expected: Actor::System,
    });

    let reg = Arc::new(FunctionRegistry::new());
    reg.register("caller", caller).unwrap();
    reg.register("probe", probe).unwrap();

    // Parent invocation carries actor = System (the FnCtx::new default).
    let ctx = FnCtx::new().with_registry(reg.clone());

    let result = reg
        .invoke("caller", &ctx, &FnBatch::new(), &Params::new())
        .await
        .unwrap();

    assert_eq!(
        result,
        QueryValue::Int(1),
        "nested ctx.call must faithfully inherit the parent's actor; with a \
         System parent the probe sees System and returns Int(1)"
    );
}
