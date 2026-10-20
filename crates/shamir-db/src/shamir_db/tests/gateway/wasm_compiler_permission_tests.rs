//! Task #607 — `ResourcePath::WasmCompiler` permission gate.
//!
//! Compiling Rust source into WASM runs a host compiler process; per the
//! user's explicit direction this is gated as a separate POSIX-style
//! permission (Execute bit on the `WasmCompiler` singleton, default `0o755`
//! mirroring `Root`) rather than folded into `FunctionNamespace`'s bare
//! Create right, and NOT via OS-level sandboxing.

use crate::access::Action;
use crate::shamir_db::ShamirDb;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

const DOUBLE_SOURCE: &str = r#"
use shamir::prelude::*;

#[shamir::function]
pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
"#;

// ============================================================================
// 1. Default meta: absent settings key -> System-owned, 0o755 (mirrors Root).
// ============================================================================

#[tokio::test]
async fn wasm_compiler_meta_defaults_to_system_0o755() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::WasmCompiler)
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::System);
    assert_eq!(meta.group, None);
    assert_eq!(meta.mode, 0o755);
}

// ============================================================================
// 2. Under the default 0o755 (everyone-execute), a non-System actor is
//    permitted `Execute` on `WasmCompiler`.
// ============================================================================

#[tokio::test]
async fn non_system_actor_permitted_execute_under_default_mode() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(123);

    let result = shamir
        .authorize_access(&actor, &ResourcePath::WasmCompiler, Action::Execute)
        .await;
    assert!(
        result.is_ok(),
        "0o755 = everyone-execute by design ('as in linux'); got {result:?}"
    );
}

// ============================================================================
// 3. After tightening to 0o700 (owner-only), the same non-owner actor is
//    denied `Execute`.
// ============================================================================

#[tokio::test]
async fn non_owner_actor_denied_execute_after_owner_only_mode() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(1);
    let actor = Actor::User(123);

    shamir
        .set_resource_meta(
            &ResourcePath::WasmCompiler,
            &ResourceMeta {
                owner,
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();

    let result = shamir
        .authorize_access(&actor, &ResourcePath::WasmCompiler, Action::Execute)
        .await;
    assert!(
        result.is_err(),
        "0o700 restricts Execute to the owner; non-owner must be denied"
    );
}

// ============================================================================
// 4. End-to-end: create_function_with_opts_as(FunctionSource::Source(..))
//    is denied BEFORE compilation under a 0o700-hardened WasmCompiler for a
//    non-owner actor, and still succeeds under the default 0o755.
// ============================================================================

#[tokio::test]
async fn create_function_from_source_denied_under_hardened_wasm_compiler() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(1);
    let actor = Actor::User(999);

    shamir
        .set_resource_meta(
            &ResourcePath::WasmCompiler,
            &ResourceMeta {
                owner,
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();

    let err = shamir
        .create_function_from_source_as("double_denied", DOUBLE_SOURCE, false, actor)
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("access denied") && msg.contains("EXECUTE"),
        "expected a permission-denial error (access denied / EXECUTE) BEFORE \
         compilation was attempted, got: {msg}"
    );
    assert!(
        !msg.contains("toolchain"),
        "expected the permission gate to fire before any toolchain check, got: {msg}"
    );
}

#[tokio::test]
async fn create_function_from_source_succeeds_under_default_wasm_compiler_mode() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let actor = Actor::User(999);

    // Default WasmCompiler mode (0o755) — actor is not owner (System is),
    // but "other" execute bit is set, so this must succeed exactly as
    // before this gate was introduced (mustn't break the existing working
    // path).
    let result = shamir
        .create_function_from_source_as("double_allowed", DOUBLE_SOURCE, false, actor)
        .await;

    match result {
        Ok(()) => {}
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("toolchain"),
                "expected either success or a toolchain-unavailable skip, got: {msg}"
            );
            eprintln!(
                "SKIP create_function_from_source_succeeds_under_default_wasm_compiler_mode: {msg}"
            );
        }
    }
}

// ============================================================================
// 5. FunctionSource::Wasm path does NOT require WasmCompiler permission at
//    all — even under a 0o700-hardened WasmCompiler owned by someone else,
//    uploading a pre-compiled WASM binary must still succeed.
// ============================================================================

#[tokio::test]
async fn create_function_from_wasm_does_not_require_wasm_compiler_permission() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(1);
    let actor = Actor::User(999);

    shamir
        .set_resource_meta(
            &ResourcePath::WasmCompiler,
            &ResourceMeta {
                owner,
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();

    // A minimal (invalid-as-a-module, but that's fine — we only care that
    // the permission gate itself is not consulted for the Wasm path) byte
    // sequence would fail at `WasmFunction::from_binary` validation, not at
    // the permission gate. Use a real precompiled wasm instead so the call
    // can succeed end-to-end without a toolchain dependency.
    let wasm = match shamir_engine::function::compile_rust_source(DOUBLE_SOURCE) {
        Ok(w) => w,
        Err(shamir_engine::function::FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!(
                "SKIP create_function_from_wasm_does_not_require_wasm_compiler_permission: {msg}"
            );
            return;
        }
        Err(e) => panic!("compile failed: {e}"),
    };

    let result = shamir
        .create_function_from_wasm_as("double_wasm_upload", &wasm, false, actor)
        .await;
    assert!(
        result.is_ok(),
        "FunctionSource::Wasm must not require WasmCompiler Execute permission; got {result:?}"
    );
}
