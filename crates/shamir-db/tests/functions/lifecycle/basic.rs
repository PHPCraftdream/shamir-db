//! Basic function lifecycle end-to-end tests: create/use/rename/drop,
//! persistence across re-open, and toolchain-gated source compilation.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_engine::function::Params;
use shamir_storage::error::DbError;
use shamir_types::types::value::QueryValue;

use super::support::{echo_wasm, init_redb_retry, make_echo_params, DOUBLE_SRC};

#[tokio::test]
async fn lifecycle_create_use_rename_use_drop() {
    let db = ShamirDb::init_memory().await.unwrap();

    // 1. Create function from pre-compiled WASM (no toolchain needed).
    db.create_function_from_wasm("echo", &echo_wasm(), false)
        .await
        .unwrap();

    // 2. Invoke it — should echo back the params map.
    let params = make_echo_params();
    let result = db.invoke_function("echo", params.clone()).await.unwrap();
    assert_eq!(result, QueryValue::Map(params.raw().clone()));

    // 3. Rename echo → echo2.
    db.rename_function("echo", "echo2").await.unwrap();

    // Old name is gone.
    assert!(db.invoke_function("echo", params.clone()).await.is_err());

    // New name works.
    let result2 = db.invoke_function("echo2", params.clone()).await.unwrap();
    assert_eq!(result2, QueryValue::Map(params.raw().clone()));

    // 4. List contains echo2, not echo.
    let names = db.list_functions().await.unwrap();
    assert!(names.contains(&"echo2".to_string()));
    assert!(!names.contains(&"echo".to_string()));

    // 5. Drop echo2 → true.
    let dropped = db.drop_function("echo2").await.unwrap();
    assert!(dropped);

    // Invoke should now fail.
    assert!(db.invoke_function("echo2", params.clone()).await.is_err());

    // Drop again → false (already gone).
    let dropped_again = db.drop_function("echo2").await.unwrap();
    assert!(!dropped_again);
}
#[tokio::test]
async fn functions_persist_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("system.db");

    // Open, create function, close.
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
            .await
            .unwrap();
        db.create_function_from_wasm("echo", &echo_wasm(), false)
            .await
            .unwrap();

        let params = make_echo_params();
        let result = db.invoke_function("echo", params.clone()).await.unwrap();
        assert_eq!(result, QueryValue::Map(params.raw().clone()));
    }

    // Re-open on the same path — function should be reloaded from catalogue.
    {
        // redb releases its exclusive file lock when the previous instance
        // drops, but that release is not synchronous on every platform
        // (Windows frees it immediately; Linux can lag a few ms behind the
        // block-scope drop). Retry the open briefly so the in-process reopen
        // is deterministic. (In production a restart releases the lock via
        // process exit; same-process reopen is a test-only pattern.)
        let db = init_redb_retry(&path).await;
        let params = make_echo_params();
        let result = db.invoke_function("echo", params.clone()).await.unwrap();
        assert_eq!(result, QueryValue::Map(params.raw().clone()));
    }
}
#[tokio::test]
async fn create_from_source_compiles() {
    let db = ShamirDb::init_memory().await.unwrap();

    let res = db
        .create_function_from_source("double", DOUBLE_SRC, false)
        .await;

    match res {
        Ok(()) => {
            let mut params = Params::new();
            params.set("n", QueryValue::Int(21));
            let result = db.invoke_function("double", params).await.unwrap();
            assert_eq!(result, QueryValue::Int(42));
        }
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!(
                "SKIP create_from_source_compiles — toolchain unavailable: {}",
                msg
            );
        }
        Err(e) => panic!("unexpected error: {}", e),
    }
}

/// The full requested e2e in ONE flow on a SOURCE-COMPILED function:
/// create → compile → use → rename → use → delete. Toolchain-gated — skips
/// cleanly when cargo / the wasm32 target is absent.
#[tokio::test]
async fn source_function_full_lifecycle() {
    let db = ShamirDb::init_memory().await.unwrap();

    // create + compile (Rust source → wasm).
    match db
        .create_function_from_source("double", DOUBLE_SRC, false)
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP source_function_full_lifecycle — toolchain unavailable: {msg}");
            return;
        }
        Err(e) => panic!("unexpected error: {e}"),
    }

    let mut params = Params::new();
    params.set("n", QueryValue::Int(21));

    // use.
    let r1 = db.invoke_function("double", params.clone()).await.unwrap();
    assert_eq!(r1, QueryValue::Int(42));

    // rename.
    db.rename_function("double", "times_two").await.unwrap();
    assert!(db.invoke_function("double", params.clone()).await.is_err());

    // use (after rename) — still the compiled artifact, no recompile.
    let r2 = db
        .invoke_function("times_two", params.clone())
        .await
        .unwrap();
    assert_eq!(r2, QueryValue::Int(42));

    // delete.
    assert!(db.drop_function("times_two").await.unwrap());
    assert!(db.invoke_function("times_two", params).await.is_err());
}
