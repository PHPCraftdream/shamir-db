use shamir_engine::function::{CreateFunctionOptions, ModuleHash, TierMode};
use shamir_types::types::value::QueryValue;

use super::support::{
    create_echo, echo_params, echo_result, echo_variant_wasm, echo_wasm, legacy_fx_hash,
    open_fjall, put_row, restart, simd_wasm, stored_row, tamper_row,
};
use crate::shamir_db::{FunctionSource, FunctionState, ShamirDb};
use crate::DbError;

async fn assert_invokes(db: &ShamirDb, name: &str) {
    let params = echo_params();
    let out = db
        .invoke_function(name, params.clone())
        .await
        .expect("invoke");
    assert_eq!(out, echo_result(&params));
}

async fn invoke_error(db: &ShamirDb, name: &str) -> String {
    db.invoke_function(name, echo_params())
        .await
        .expect_err("invoke must fail closed")
        .to_string()
}

#[tokio::test]
async fn legacy_row_with_matching_hash_loads_and_boot_does_not_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let echo = echo_wasm();
    let db = open_fjall(&path).await;
    put_row(
        &db,
        "n",
        &echo,
        vec![("wasm_hash", QueryValue::Str(legacy_fx_hash(&echo)))],
    )
    .await;

    let db = restart(db, &path).await;

    let d = db.describe_function("n").await.expect("describe");
    assert_eq!(d.state, FunctionState::Loaded);
    assert_eq!(d.hash, Some(ModuleHash::of(&echo).to_hex()));
    assert_invokes(&db, "n").await;
    assert!(stored_row(&db, "n").await.get("module_hash").is_none());
}

#[tokio::test]
async fn tampered_legacy_row_is_corrupt_with_unknown_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    put_row(
        &db,
        "n",
        &echo_wasm(),
        vec![("wasm_hash", QueryValue::Str("0000000000000000".into()))],
    )
    .await;

    let db = restart(db, &path).await;

    let d = db.describe_function("n").await.expect("describe");
    assert!(
        matches!(d.state, FunctionState::Corrupt { .. }),
        "{:?}",
        d.state
    );
    assert_eq!(d.hash, None);
    assert!(invoke_error(&db, "n").await.contains("corrupt"));
}

#[tokio::test]
async fn row_without_any_hash_loads_with_unknown_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    put_row(&db, "n", &echo_wasm(), Vec::new()).await;

    let db = restart(db, &path).await;

    let d = db.describe_function("n").await.expect("describe");
    assert_eq!(d.state, FunctionState::Loaded);
    assert_eq!(d.hash, None);
    assert_invokes(&db, "n").await;
}

#[tokio::test]
async fn module_hash_tampering_is_corrupt_and_replace_repairs_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let echo = echo_wasm();
    let variant = echo_variant_wasm();
    let db = open_fjall(&path).await;
    create_echo(&db, "n").await;
    tamper_row(&db, "n", &variant).await;

    let db = restart(db, &path).await;

    let d = db.describe_function("n").await.expect("describe");
    assert_eq!(
        d.state,
        FunctionState::Corrupt {
            expected: ModuleHash::of(&echo).to_hex(),
            actual: ModuleHash::of(&variant).to_hex(),
        }
    );
    assert!(invoke_error(&db, "n").await.contains("corrupt"));
    assert!(db.export_function("n").await.is_err());

    db.create_function_from_wasm("n", &echo, true)
        .await
        .expect("replace repairs");
    assert_eq!(
        db.describe_function("n").await.expect("describe").state,
        FunctionState::Loaded
    );
    assert_invokes(&db, "n").await;

    let db = restart(db, &path).await;
    assert_eq!(
        db.describe_function("n").await.expect("describe").state,
        FunctionState::Loaded
    );
}

#[tokio::test]
async fn corrupt_function_can_be_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    create_echo(&db, "n").await;
    create_echo(&db, "keep").await;
    tamper_row(&db, "n", &echo_variant_wasm()).await;
    let db = restart(db, &path).await;

    assert!(db.drop_function("n").await.expect("drop"));

    let err = db.describe_function("n").await.expect_err("dropped");
    assert!(matches!(err, DbError::NotFound(_)), "got: {err}");
    assert_eq!(
        db.describe_function("keep").await.expect("keep").state,
        FunctionState::Loaded
    );
}

#[tokio::test]
async fn simd_row_is_unloadable_visible_and_repairable_by_replace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    put_row(&db, "n", &simd_wasm(), Vec::new()).await;

    let db = restart(db, &path).await;

    let d = db.describe_function("n").await.expect("describe");
    match &d.state {
        FunctionState::Unloadable { reason } => assert!(!reason.is_empty()),
        other => panic!("expected Unloadable, got {other:?}"),
    }
    assert!(d.tier_report.is_none());
    assert!(db
        .list_functions()
        .await
        .expect("list")
        .contains(&"n".to_string()));
    assert!(invoke_error(&db, "n").await.contains("unavailable"));

    let db = restart(db, &path).await;
    assert!(matches!(
        db.describe_function("n").await.expect("describe").state,
        FunctionState::Unloadable { .. }
    ));
    assert!(stored_row(&db, "n").await.get("wasm_b64").is_some());

    db.create_function_from_wasm("n", &echo_wasm(), true)
        .await
        .expect("replace repairs");
    assert_eq!(
        db.describe_function("n").await.expect("describe").state,
        FunctionState::Loaded
    );
    assert_invokes(&db, "n").await;
}

#[tokio::test]
async fn stored_optimize_survives_restart_and_absent_means_auto() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    let opts = CreateFunctionOptions {
        optimize: TierMode::Never,
        ..CreateFunctionOptions::default()
    };
    db.create_function_with_opts("never", FunctionSource::Wasm(&echo_wasm()), opts)
        .await
        .expect("create");
    put_row(&db, "bare", &echo_wasm(), Vec::new()).await;

    let db = restart(db, &path).await;

    let never = db.describe_function("never").await.expect("describe");
    assert_eq!(never.optimize, TierMode::Never);
    assert_eq!(never.tier_report.expect("tier").mode, TierMode::Never);
    let bare = db.describe_function("bare").await.expect("describe");
    assert_eq!(bare.optimize, TierMode::Auto);
    assert_eq!(bare.tier_report.expect("tier").mode, TierMode::Auto);
}

#[tokio::test]
async fn undecodable_base64_row_boots_unloadable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    put_row(
        &db,
        "n",
        &[],
        vec![("wasm_b64", QueryValue::Str("***".into()))],
    )
    .await;

    let db = restart(db, &path).await;

    match db.describe_function("n").await.expect("describe").state {
        FunctionState::Unloadable { reason } => assert!(reason.contains("base64"), "{reason}"),
        other => panic!("expected Unloadable, got {other:?}"),
    }
}
