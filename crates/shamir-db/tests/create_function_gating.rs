//! Per-field wire-gating access-control test for `create_function_with_opts_as`
//! (task #554, brief §2).
//!
//! The `secret_grants` gate (`Manage(Root)`) cannot be exercised over the
//! wire: a non-superuser session is rejected by the coarse admin gate
//! (`permission_denied`) before it ever reaches `handle_create_function`,
//! and a superuser session maps to `Actor::System` which bypasses
//! `authorize_access` unconditionally. So the `Manage(Root)` half of the
//! gate is verified here, in-process, against a non-System `Actor::User`
//! who has `Create(FunctionNamespace)` (the default `0o777` grants it to
//! everyone) but NOT `Manage(Root)` (Root's default owner is `System`;
//! `Manage` is owner-only).

use shamir_db::shamir_db::FunctionSource;
use shamir_db::ShamirDb;
use shamir_engine::function::{CreateFunctionOptions, Security, Visibility};
use shamir_types::access::{principal64_from_username, Actor};

/// Identity-echo WAT (same minimal slice-2 ABI module used in
/// `functions_lifecycle.rs`) — compiled to WASM so we can create a real
/// function without the cargo/wasm32 toolchain.
const ECHO_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
      (i64.extend_i32_u (local.get $len)))))
"#;

fn echo_wasm() -> Vec<u8> {
    wat::parse_str(ECHO_WAT).unwrap()
}

/// Non-empty `secret_grants` WITHOUT `Manage(Root)` → access denied, even
/// though the actor CAN create functions in general (FunctionNamespace's
/// default `0o777` grants `Create` to everyone). Confirms the gate is the
/// ADDITIONAL `Manage(Root)` requirement, not a side-effect of the
/// existing `Create(FunctionNamespace)` check.
#[tokio::test]
async fn secret_grants_without_root_manage_denied() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // A regular user. Root's default meta is { owner: System, mode: 0o755 },
    // so alice (Other) has Execute (traverse) but is NOT the owner →
    // `Manage(Root)` fails. FunctionNamespace's default meta is
    // `ResourceMeta::open()` = { owner: System, mode: 0o777 }, so alice
    // (Other) has Create via the Write bit.
    let alice = Actor::User(principal64_from_username("alice"));

    let opts = CreateFunctionOptions {
        replace: false,
        visibility: Visibility::Private,
        security: Security::Invoker,
        secret_grants: vec!["ADMIN_DB_PASSWORD".to_string()],
        net_grants: Vec::new(),
    };
    let err = shamir
        .create_function_with_opts_as("exfil_fn", FunctionSource::Wasm(&echo_wasm()), opts, alice)
        .await
        .expect_err("secret_grants without Manage(Root) must be denied");

    // The access-denied nature must be visible in the error. The facade
    // wraps the AccessError in DbError::Function, whose Display prefixes
    // "Function error: " — check the inner "access denied" substring and
    // that it names MANAGE on the root path.
    let msg = err.to_string();
    assert!(
        msg.contains("access denied"),
        "expected an access-denied error, got: {msg}"
    );
    assert!(
        msg.contains("MANAGE"),
        "expected the denial to name the MANAGE action, got: {msg}"
    );

    // And no function must have been persisted.
    assert!(shamir.function_meta("exfil_fn").is_none());
}

/// Control: the SAME actor CAN create a plain function (no secret_grants)
/// — the `Manage(Root)` gate fires only when `secret_grants` is non-empty.
/// This confirms the gate is per-field, not a blanket block on non-System
/// function creation.
#[tokio::test]
async fn plain_create_without_root_manage_succeeds_for_non_system_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let alice = Actor::User(principal64_from_username("alice"));

    let opts = CreateFunctionOptions {
        replace: false,
        visibility: Visibility::Private,
        security: Security::Invoker,
        secret_grants: Vec::new(),
        net_grants: Vec::new(),
    };
    shamir
        .create_function_with_opts_as("plain_fn", FunctionSource::Wasm(&echo_wasm()), opts, alice)
        .await
        .expect("plain create (no secret_grants) must succeed for a non-System actor");
    assert!(shamir.function_meta("plain_fn").is_some());
}
