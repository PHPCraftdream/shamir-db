//! Regression tests for the terminal `shamir_host::raise_error` import.
use crate::wasm::bindings::decode_guest_error;
use crate::{
    FnBatch, FnCtx, FunctionError, FunctionRegistry, Params, ShamirFunction, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

fn wat(kind: i32, ptr: i32, len: i32, message: &str) -> String {
    let data = message
        .as_bytes()
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    let bump_ptr = 8192;
    format!("(module (import \"shamir_host\" \"raise_error\" (func $raise (param i32 i32 i32))) (memory (export \"memory\") 1) (data (i32.const 0) \"{data}\") (global $bump (mut i32) (i32.const {bump_ptr})) (func (export \"shamir_alloc\") (param $size i32) (result i32) (local $ptr i32) (local.set $ptr (global.get $bump)) (global.set $bump (i32.add (global.get $bump) (local.get $size))) (local.get $ptr)) (func (export \"shamir_call\") (param i32 i32) (result i64) i32.const {kind} i32.const {ptr} i32.const {len} call $raise i64.const 4294967296))")
}

fn invoke_wat(source: &str, ctx: &FnCtx) -> Result<QueryValue, FunctionError> {
    let engine =
        Arc::new(WasmEngine::new().map_err(|error| FunctionError::Compute(error.to_string()))?);
    let function = WasmFunction::from_wat(engine, source, WasmLimits::default())?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(function.call(ctx, &FnBatch::new(), &Params::new()))
}

#[test]
fn decode_guest_error_caps_char_boundaries() {
    let at_cap = vec![b'a'; 1024];
    assert!(
        matches!(decode_guest_error(1, &at_cap, 1024), FunctionError::User(message) if message.len() == 1024)
    );
    let over_cap = vec![b'a'; 1025];
    assert!(
        matches!(decode_guest_error(1, &over_cap, 1025), FunctionError::User(message) if message == format!("{} [truncated]", "a".repeat(1024)))
    );
    let controls = vec![0u8; 1024];
    assert!(matches!(
        decode_guest_error(1, &controls, 1024),
        FunctionError::User(message) if message.len() == 1023 && message.chars().all(|character| character == '\u{fffd}')
    ));
    let invalid_run = [0xff; 1024];
    assert!(matches!(
        decode_guest_error(1, &invalid_run, invalid_run.len()),
        FunctionError::User(message) if message.len() == 1023 && message.chars().all(|character| character == '\u{fffd}')
    ));
    let mut boundary = vec![b'a'; 1023];
    boundary.extend_from_slice(&[0x00, b'z']);
    assert!(matches!(
        decode_guest_error(1, &boundary, boundary.len()),
        FunctionError::User(message) if message == format!("{} [truncated]", "a".repeat(1023))
    ));
    let mut split = vec![b'a'; 1023];
    split.extend_from_slice("é".as_bytes());
    assert!(
        matches!(decode_guest_error(1, &split, 1025), FunctionError::User(message) if message == format!("{} [truncated]", "a".repeat(1023)))
    );
}

#[test]
fn rv_sdk_6d2497ca_raise_user_maps_to_user() {
    assert!(
        matches!(invoke_wat(&wat(1, 0, 12, "user failure"), &FnCtx::new()), Err(FunctionError::User(message)) if message == "user failure")
    );
}
#[test]
fn rv_sdk_6d2497ca_codec_host_invalid_arg_map_to_compute_with_prefix() {
    for (kind, expected) in [
        (2, "guest invalid argument: user failure"),
        (3, "guest codec: user failure"),
        (4, "guest host: user failure"),
    ] {
        assert!(
            matches!(invoke_wat(&wat(kind, 0, 12, "user failure"), &FnCtx::new()), Err(FunctionError::Compute(message)) if message == format!("shamir_call: compute error: {expected}"))
        );
    }
}
#[test]
fn rv_sdk_6d2497ca_unknown_kind_compute() {
    for kind in [0, 99, -1] {
        assert!(matches!(
            invoke_wat(&wat(kind, 0, 1, "x"), &FnCtx::new()),
            Err(FunctionError::Compute(message)) if message == format!("shamir_call: compute error: unknown guest error kind {kind}: x")
        ));
    }
}
#[test]
fn rv_sdk_6d2497ca_old_guest_unreachable_stays_compute() {
    let source = r#"(module (memory (export "memory") 1) (func (export "shamir_alloc") (param i32) (result i32) i32.const 0) (func (export "shamir_call") (param i32 i32) (result i64) unreachable))"#;
    assert!(matches!(
        invoke_wat(source, &FnCtx::new()),
        Err(FunctionError::Compute(_))
    ));
}
#[test]
fn rv_sdk_6d2497ca_import_unused_success_untouched() {
    let source = r#"(module (import "shamir_host" "raise_error" (func (param i32 i32 i32))) (memory (export "memory") 1) (data (i32.const 4096) "\80") (func (export "shamir_alloc") (param i32) (result i32) i32.const 8192) (func (export "shamir_call") (param i32 i32) (result i64) i64.const 17592186044417))"#;
    assert!(
        matches!(invoke_wat(source, &FnCtx::new()), Ok(QueryValue::Map(map)) if map.is_empty())
    );
}
#[test]
fn rv_sdk_6d2497ca_raise_then_success_code_never_runs() {
    let source = r#"(module (import "shamir_host" "raise_error" (func $raise (param i32 i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "failure") (data (i32.const 16) "\80") (func (export "shamir_alloc") (param i32) (result i32) i32.const 8192) (func (export "shamir_call") (param i32 i32) (result i64) i32.const 1 i32.const 0 i32.const 7 call $raise i64.const 68719476737))"#;
    assert!(matches!(
        invoke_wat(source, &FnCtx::new()),
        Err(FunctionError::User(_))
    ));
}
#[test]
fn rv_sdk_6d2497ca_oversized_len_truncated_to_cap() {
    let message = "a".repeat(2048);
    assert!(
        matches!(invoke_wat(&wat(1, 0, 2048, &message), &FnCtx::new()), Err(FunctionError::User(value)) if value == format!("{} [truncated]", "a".repeat(1024)))
    );
}
#[test]
fn rv_sdk_6d2497ca_prefix_oob_compute() {
    assert!(matches!(
        invoke_wat(&wat(1, 65530, 12, "x"), &FnCtx::new()),
        Err(FunctionError::Compute(message)) if message == "shamir_call: compute error: raise_error: guest memory out of bounds"
    ));
}
#[test]
fn rv_sdk_6d2497ca_negative_ptr_len_compute() {
    assert!(matches!(
        invoke_wat(&wat(1, -1, 1, "x"), &FnCtx::new()),
        Err(FunctionError::Compute(message)) if message == "shamir_call: compute error: raise_error: negative pointer or length"
    ));
    assert!(matches!(
        invoke_wat(&wat(1, 0, -1, "x"), &FnCtx::new()),
        Err(FunctionError::Compute(message)) if message == "shamir_call: compute error: raise_error: negative pointer or length"
    ));
}
#[test]
fn rv_sdk_6d2497ca_invalid_utf8_and_control_chars_replaced() {
    let source = r#"(module (import "shamir_host" "raise_error" (func $raise (param i32 i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "\61\ff\00\62") (func (export "shamir_alloc") (param i32) (result i32) i32.const 128) (func (export "shamir_call") (param i32 i32) (result i64) i32.const 1 i32.const 0 i32.const 4 call $raise i64.const 4294967296))"#;
    assert!(
        matches!(invoke_wat(source, &FnCtx::new()), Err(FunctionError::User(message)) if message == "a��b")
    );
}

#[test]
fn rv_sdk_6d2497ca_nested_child_user_propagates() {
    let engine = Arc::new(WasmEngine::new().unwrap());
    let parent_source = r#"(module (import "shamir_host" "call" (func $call (param i32 i32 i32 i32) (result i64))) (memory (export "memory") 1) (data (i32.const 0) "child") (data (i32.const 16) "\80") (global $bump (mut i32) (i32.const 1024)) (func (export "shamir_alloc") (param $size i32) (result i32) (local $ptr i32) (local.set $ptr (global.get $bump)) (global.set $bump (i32.add (global.get $bump) (local.get $size))) (local.get $ptr)) (func (export "shamir_call") (param i32 i32) (result i64) (call $call (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 1))))"#;
    let child_source = r#"(module (import "shamir_host" "raise_error" (func $raise (param i32 i32 i32))) (memory (export "memory") 1) (data (i32.const 0) "nested failure") (func (export "shamir_alloc") (param i32) (result i32) i32.const 128) (func (export "shamir_call") (param i32 i32) (result i64) i32.const 1 i32.const 0 i32.const 14 call $raise i64.const 4294967296))"#;
    let parent = Arc::new(
        WasmFunction::from_wat(engine.clone(), parent_source, WasmLimits::default()).unwrap(),
    );
    let child =
        Arc::new(WasmFunction::from_wat(engine, child_source, WasmLimits::default()).unwrap());
    let registry = Arc::new(FunctionRegistry::new());
    registry.register("parent", parent).unwrap();
    registry.register("child", child).unwrap();
    let ctx = FnCtx::new().with_registry(registry.clone());
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(registry.invoke("parent", &ctx, &FnBatch::new(), &Params::new()));
    assert!(matches!(result, Err(FunctionError::User(message)) if message == "nested failure"));
}
