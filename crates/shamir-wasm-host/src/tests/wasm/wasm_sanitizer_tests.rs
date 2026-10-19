use crate::{
    test_linker_and_store, FnBatch, FnCtx, Params, ShamirFunction, WasmEngine, WasmFunction,
    WasmLimits,
};
use crate::{verify_wasm_module, SANCTIONED_HOST_IMPORTS};
use shamir_collections::TFxSet;
use std::sync::Arc;

/// Minimal valid ABI (memory + `shamir_alloc` + `shamir_call`) with no
/// imports at all — the baseline every positive-case WAT below extends.
const NO_IMPORTS_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (i32.const 1024)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.const 0)
  )
)
"#;

/// A WAT module importing a bogus `("evil", "syscall")` pair — well-formed
/// WAT/wasm syntax wasmtime itself would happily attempt to instantiate
/// (it is only rejected by the *linker's* import resolution, or — with
/// this change — by the sanitizer first). Used to prove the sanitizer's
/// own check fires independently of wasmtime's linker-resolution failure,
/// per the brief's requirement to isolate the sanitizer's own behaviour.
const UNSANCTIONED_MODULE_IMPORT_WAT: &str = r#"
(module
  (import "evil" "syscall" (func $syscall (param i32) (result i32)))
  (memory (export "memory") 2)
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (i32.const 1024)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.const 0)
  )
)
"#;

/// A WAT module importing an unsanctioned NAME under the CORRECT module
/// (`shamir_host`) — proves the allowlist is name-scoped, not just
/// module-scoped (a smuggled `shamir_host::backdoor` must be rejected even
/// though the module string matches the sanctioned namespace).
const UNSANCTIONED_NAME_UNDER_SANCTIONED_MODULE_WAT: &str = r#"
(module
  (import "shamir_host" "backdoor" (func $backdoor (param i32) (result i32)))
  (memory (export "memory") 2)
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (i32.const 1024)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.const 0)
  )
)
"#;

/// A WAT module using every sanctioned import at once, with arities matched
/// exactly to each host function's real Rust signature (`host_batch.rs`,
/// `host_globals.rs`, `host_call.rs`, `host_db.rs`, `host_http.rs`): the
/// four sync imports take `(i32,i32,i32,i32)`/`(i32,i32) -> i64`; `call`
/// and the two-`(ptr,len)`-argument imports (`db_execute`, `http_fetch`)
/// take `(i32,i32,i32,i32) -> i64`/`(i32,i32) -> i64` respectively;
/// `db_get`/`db_insert`/`db_query` take `(i32,i32,i32,i32) -> i64`. Only
/// the import section shape matters for this test — the functions are
/// never actually called.
const ALL_SANCTIONED_IMPORTS_WAT: &str = r#"
(module
  (import "shamir_host" "batch_put" (func $batch_put (param i32 i32 i32 i32)))
  (import "shamir_host" "batch_get" (func $batch_get (param i32 i32) (result i64)))
  (import "shamir_host" "global_set" (func $global_set (param i32 i32 i32 i32)))
  (import "shamir_host" "global_get" (func $global_get (param i32 i32) (result i64)))
  (import "shamir_host" "call" (func $call (param i32 i32 i32 i32) (result i64)))
  (import "shamir_host" "db_get" (func $db_get (param i32 i32 i32 i32) (result i64)))
  (import "shamir_host" "db_insert" (func $db_insert (param i32 i32 i32 i32) (result i64)))
  (import "shamir_host" "db_query" (func $db_query (param i32 i32 i32 i32) (result i64)))
  (import "shamir_host" "db_execute" (func $db_execute (param i32 i32) (result i64)))
  (import "shamir_host" "http_fetch" (func $http_fetch (param i32 i32) (result i64)))

  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  ;; Echoes back the same `[ptr, len)` region — same identity-ABI pattern
  ;; as `wasm_tests.rs`'s `IDENTITY_WAT`. None of the imported host
  ;; functions above are actually called; only the import SECTION shape
  ;; (module+name+type) matters for the sanitizer's allowlist check.
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

/// A minimal, well-formed WASM *component* (component-model encoding,
/// distinct from a core module) wrapping a trivial empty core module.
/// Used to prove the sanitizer explicitly rejects the component encoding
/// rather than silently no-op-ing on it (adversarial-review finding: the
/// component-specific `Payload` variants `wasmparser`'s component-model
/// feature yields never match `Payload::ImportSection`, so without an
/// explicit `Encoding::Component` check this would otherwise scan clean).
const MINIMAL_COMPONENT_WAT: &str = r#"
(component
  (core module $m
    (func (export "f") (result i32) (i32.const 0))
  )
)
"#;

// ── Direct `verify_wasm_module` unit tests ───────────────────────────────

#[test]
fn no_imports_module_passes_sanitizer() {
    let bytes = wat::parse_str(NO_IMPORTS_WAT).unwrap();
    assert!(verify_wasm_module(&bytes).is_ok());
}

#[test]
fn component_encoded_artifact_rejected_by_sanitizer() {
    let bytes = wat::parse_str(MINIMAL_COMPONENT_WAT).unwrap();
    let err = verify_wasm_module(&bytes).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("component"),
        "error should name the rejected component encoding, got: {msg}"
    );
}

#[test]
fn all_sanctioned_imports_pass_sanitizer() {
    let bytes = wat::parse_str(ALL_SANCTIONED_IMPORTS_WAT).unwrap();
    assert!(verify_wasm_module(&bytes).is_ok());
}

#[test]
fn unsanctioned_module_name_rejected_by_sanitizer() {
    let bytes = wat::parse_str(UNSANCTIONED_MODULE_IMPORT_WAT).unwrap();
    let err = verify_wasm_module(&bytes).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("evil") && msg.contains("syscall"),
        "error should name the rejected import, got: {msg}"
    );
}

#[test]
fn unsanctioned_name_under_sanctioned_module_rejected_by_sanitizer() {
    let bytes = wat::parse_str(UNSANCTIONED_NAME_UNDER_SANCTIONED_MODULE_WAT).unwrap();
    let err = verify_wasm_module(&bytes).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("shamir_host") && msg.contains("backdoor"),
        "error should name the rejected import, got: {msg}"
    );
}

// ── End-to-end: sanitizer runs before wasmtime instantiation ─────────────

/// RED before the fix / GREEN after: a well-formed WAT module (valid
/// syntax wasmtime itself would attempt to compile+instantiate) with an
/// unsanctioned import must be rejected — and specifically rejected with
/// the sanitizer's own error text, proving the sanitizer's check fired
/// (as opposed to merely wasmtime's linker resolution failing later with
/// its own, different error message).
#[test]
fn wasm_function_from_wat_rejects_unsanctioned_import_via_sanitizer() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let result = WasmFunction::from_wat(
        engine,
        UNSANCTIONED_MODULE_IMPORT_WAT,
        WasmLimits::default(),
    );
    let err = match result {
        Ok(_) => panic!("expected the sanitizer to reject this module"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("wasm sanitizer"),
        "expected the sanitizer (not wasmtime's own linker resolution) to reject this module \
         first, got: {err}"
    );
}

#[test]
fn wasm_function_from_binary_rejects_unsanctioned_import_via_sanitizer() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let bytes = wat::parse_str(UNSANCTIONED_MODULE_IMPORT_WAT).unwrap();
    let result = WasmFunction::from_binary(engine, &bytes, WasmLimits::default());
    let err = match result {
        Ok(_) => panic!("expected the sanitizer to reject this module"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("wasm sanitizer"),
        "expected the sanitizer to reject this module first, got: {err}"
    );
}

/// No regression on the happy path: a legitimate module using only
/// sanctioned imports still loads AND invokes successfully end-to-end
/// (extends the existing `wasm_identity_roundtrips_params` pattern from
/// `wasm_tests.rs`, but exercises every sanctioned import name in the
/// module's import section rather than none).
#[tokio::test]
async fn legitimate_module_with_sanctioned_imports_still_loads_and_invokes() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let wf = Arc::new(
        WasmFunction::from_wat(engine, ALL_SANCTIONED_IMPORTS_WAT, WasmLimits::default()).unwrap(),
    );
    let result = wf
        .call(&FnCtx::new(), &FnBatch::new(), &Params::new())
        .await;
    assert!(
        result.is_ok(),
        "legitimate module with only sanctioned imports must still invoke: {result:?}"
    );
}

// ── Sync check: sanitizer allowlist vs. build_instance_pre's real ────────
// ── registrations                                                     ────

/// Mechanically enforces that [`SANCTIONED_HOST_IMPORTS`] cannot silently
/// drift from what `build_instance_pre`/`build_linker` actually registers
/// on the `wasmtime::Linker`. Builds the REAL linker (same code path
/// production uses) and enumerates its registered `(module, name)` pairs
/// via `Linker::iter`, then asserts set-equality against the allowlist
/// constant in both directions:
/// - every linker registration is present in the allowlist (else the
///   linker's surface silently grew beyond what the sanitizer permits),
/// - every allowlist entry is actually registered on the linker (else the
///   sanitizer allows a name the linker doesn't even resolve, which the
///   `no_imports`/`all_sanctioned_imports` tests above wouldn't catch by
///   themselves for a hypothetical stale/extra allowlist entry).
#[test]
fn sanctioned_list_matches_linker_registrations() {
    let engine = WasmEngine::new().unwrap();
    let (linker, mut store) = test_linker_and_store(&engine).unwrap();

    let registered: TFxSet<(String, String)> = linker
        .iter(&mut store)
        .map(|(module, name, _extern)| (module.to_string(), name.to_string()))
        .collect();

    let allowlisted: TFxSet<(String, String)> = SANCTIONED_HOST_IMPORTS
        .iter()
        .map(|(m, n)| (m.to_string(), n.to_string()))
        .collect();

    let extra_on_linker: Vec<_> = registered.difference(&allowlisted).collect();
    assert!(
        extra_on_linker.is_empty(),
        "linker registers import(s) NOT in SANCTIONED_HOST_IMPORTS — the sanitizer's allowlist \
         has drifted behind the linker's actual surface: {extra_on_linker:?}"
    );

    let extra_in_allowlist: Vec<_> = allowlisted.difference(&registered).collect();
    assert!(
        extra_in_allowlist.is_empty(),
        "SANCTIONED_HOST_IMPORTS lists import(s) the linker does NOT actually register — stale \
         allowlist entries: {extra_in_allowlist:?}"
    );
}
