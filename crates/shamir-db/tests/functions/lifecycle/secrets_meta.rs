//! Secret-grant enforcement for env.* globals, guest global_set
//! protection, and function metadata roundtrip.

use shamir_db::shamir_db::{FunctionSource, SystemStoreConfig};
use shamir_db::ShamirDb;
use shamir_engine::function::{CreateFunctionOptions, Params, Security, Visibility};
use shamir_storage::error::DbError;
use shamir_types::types::value::QueryValue;

use super::support::{echo_wasm, init_redb_retry};

// ── Slice 9: secret-grant enforcement + function metadata ────────────

/// Source that reads `ctx.global_get("env.SHAMIR_S9_SECRET")` and returns
/// the value (or Null if absent).
const ENV_READER_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn reader(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    Ok(ctx.global_get("env.SHAMIR_S9_SECRET").unwrap_or(Value::Null))
}
"#;

/// Source that reads a non-env global to prove non-env globals are ungated.
const NON_ENV_READER_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn non_env_reader(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    Ok(ctx.global_get("my_cache_key").unwrap_or(Value::Null))
}
"#;

/// A function may read an `env.*` global ONLY if the env name is in its
/// `secret_grants`. Without a grant the global looks absent (Null).
/// With a grant the function sees the real value. Non-`env.` globals
/// remain freely readable regardless of grants.
///
/// Toolchain-gated — skips cleanly when cargo / wasm32 target is absent.
#[tokio::test]
async fn secret_grant_gates_env_read() {
    // Set a unique env var that the default policy (SHAMIR_* prefix) picks up.
    std::env::set_var("SHAMIR_S9_SECRET", "topsecret");
    let db = ShamirDb::init_memory().await.unwrap();
    // Sanity: the env var was seeded into globals.
    assert_eq!(
        db.globals().get("env.SHAMIR_S9_SECRET"),
        Some(QueryValue::Str("topsecret".to_string()))
    );

    // Also set a non-env global to prove non-env access is ungated.
    db.globals()
        .set("my_cache_key", QueryValue::Str("cache_val".to_string()));

    // ── Compile the env reader ──
    match db
        .create_function_from_source("reader", ENV_READER_SRC, false)
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP secret_grant_gates_env_read — toolchain unavailable: {msg}");
            std::env::remove_var("SHAMIR_S9_SECRET");
            return;
        }
        Err(e) => {
            std::env::remove_var("SHAMIR_S9_SECRET");
            panic!("compile reader failed: {e}");
        }
    }

    // Compile the non-env reader.
    let compiled_non_env = match db
        .create_function_from_source("non_env_reader", NON_ENV_READER_SRC, false)
        .await
    {
        Ok(()) => true,
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP non_env_reader — toolchain unavailable: {msg}");
            false
        }
        Err(e) => panic!("compile non_env_reader failed: {e}"),
    };

    // ── Invoke WITHOUT grant (default empty) → must see Null ──
    let result = db.invoke_function("reader", Params::new()).await.unwrap();
    assert_eq!(
        result,
        QueryValue::Null,
        "without secret_grant, env.SHAMIR_S9_SECRET must look absent"
    );

    // ── Non-env global is freely readable even with empty grants ──
    if compiled_non_env {
        let result = db
            .invoke_function("non_env_reader", Params::new())
            .await
            .unwrap();
        assert_eq!(
            result,
            QueryValue::Str("cache_val".to_string()),
            "non-env globals must be readable without any secret_grant"
        );
    }

    // ── Recreate WITH grant → must see the real value ──
    let opts = CreateFunctionOptions {
        replace: true,
        visibility: Visibility::Private,
        security: Security::Invoker,
        secret_grants: vec!["SHAMIR_S9_SECRET".to_string()],
        net_grants: Vec::new(),
    };
    db.create_function_with_opts("reader", FunctionSource::Source(ENV_READER_SRC), opts)
        .await
        .unwrap();

    let result = db.invoke_function("reader", Params::new()).await.unwrap();
    assert_eq!(
        result,
        QueryValue::Str("topsecret".to_string()),
        "with secret_grant, env.SHAMIR_S9_SECRET must be readable"
    );

    std::env::remove_var("SHAMIR_S9_SECRET");
}

/// Source that writes an `env.*` global via `global_set`, then immediately
/// reads it back (WITH a matching `secret_grants` entry, so any observed
/// change is attributable to the write attempt, not the read gate) and
/// returns what it sees.
const ENV_WRITER_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn writer(ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    ctx.global_set("env.SHAMIR_S9_WRITE_SECRET", Value::Str("pwned".to_string()));
    Ok(ctx.global_get("env.SHAMIR_S9_WRITE_SECRET").unwrap_or(Value::Null))
}
"#;

/// A guest `global_set("env.SOME_KEY", ...)` must NOT overwrite an
/// OS-seeded `env.*` value — no guest may ever write into the `env.*`
/// namespace, regardless of `secret_grants` (finding 2, task #544).
///
/// The writer function is granted `secret_grants: ["SHAMIR_S9_WRITE_SECRET"]`
/// (so it CAN read the secret) and attempts to overwrite it via
/// `global_set`. Before the fix this silently succeeds and corrupts the
/// seeded value for the rest of the process; after the fix the write
/// attempt fails (the host import traps) and the seeded value survives,
/// verified independently via `db.globals()`.
///
/// Toolchain-gated — skips cleanly when cargo / wasm32 target is absent.
#[tokio::test]
async fn global_set_cannot_overwrite_seeded_env_value() {
    std::env::set_var("SHAMIR_S9_WRITE_SECRET", "original-secret");
    let db = ShamirDb::init_memory().await.unwrap();
    assert_eq!(
        db.globals().get("env.SHAMIR_S9_WRITE_SECRET"),
        Some(QueryValue::Str("original-secret".to_string())),
        "sanity: the env var must be seeded before the guest tries to overwrite it"
    );

    let opts = CreateFunctionOptions {
        replace: false,
        visibility: Visibility::Private,
        security: Security::Invoker,
        secret_grants: vec!["SHAMIR_S9_WRITE_SECRET".to_string()],
        net_grants: Vec::new(),
    };
    match db
        .create_function_with_opts("writer", FunctionSource::Source(ENV_WRITER_SRC), opts)
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!(
                "SKIP global_set_cannot_overwrite_seeded_env_value — toolchain unavailable: {msg}"
            );
            std::env::remove_var("SHAMIR_S9_WRITE_SECRET");
            return;
        }
        Err(e) => {
            std::env::remove_var("SHAMIR_S9_WRITE_SECRET");
            panic!("compile writer failed: {e}");
        }
    }

    // The write attempt must fail closed (the host import traps): the
    // function invocation itself must return an error, NOT a successful
    // "pwned" result.
    let result = db.invoke_function("writer", Params::new()).await;
    assert!(
        result.is_err(),
        "global_set into env.* must trap the invocation, got: {result:?}"
    );

    // The seeded value must have survived the write attempt untouched —
    // verified independently through the shared GlobalVars, not just the
    // function's own (failed) return value.
    assert_eq!(
        db.globals().get("env.SHAMIR_S9_WRITE_SECRET"),
        Some(QueryValue::Str("original-secret".to_string())),
        "a guest global_set must never corrupt a seeded env.* value"
    );

    std::env::remove_var("SHAMIR_S9_WRITE_SECRET");
}

/// Function metadata (visibility, security, secret_grants) persists across
/// a re-open on a redb-backed store.
#[tokio::test]
async fn function_meta_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("system_meta.db");

    // Open, create function with explicit metadata, close.
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
            .await
            .unwrap();
        let opts = CreateFunctionOptions {
            replace: false,
            visibility: Visibility::Public,
            security: Security::Definer,
            secret_grants: vec!["FOO".to_string(), "BAR".to_string()],
            net_grants: Vec::new(),
        };
        db.create_function_with_opts("echo", FunctionSource::Wasm(&echo_wasm()), opts)
            .await
            .unwrap();

        // Verify metadata is available right after creation.
        let meta = db.function_meta("echo").unwrap();
        assert_eq!(meta.visibility, Visibility::Public);
        assert_eq!(meta.security, Security::Definer);
        assert_eq!(
            meta.secret_grants,
            vec!["FOO".to_string(), "BAR".to_string()]
        );
    }

    // Re-open — metadata should be reloaded from the persisted catalogue.
    {
        let db = init_redb_retry(&path).await;
        let meta = db.function_meta("echo").expect("meta must survive reopen");
        assert_eq!(meta.visibility, Visibility::Public);
        assert_eq!(meta.security, Security::Definer);
        assert_eq!(
            meta.secret_grants,
            vec!["FOO".to_string(), "BAR".to_string()]
        );
    }
}
