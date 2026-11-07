use shamir_engine::function::TierMode;

use super::support::{
    create_echo, echo_params, echo_result, echo_variant_wasm, echo_wasm, open_fjall, put_row,
    restart, simd_wasm, stored_row, tamper_row, without_key,
};
use crate::access::Actor;
use crate::shamir_db::{FunctionState, ShamirDb};
use crate::DbError;

async fn assert_invokes(db: &ShamirDb, name: &str) {
    let params = echo_params();
    let out = db
        .invoke_function(name, params.clone())
        .await
        .expect("invoke");
    assert_eq!(out, echo_result(&params));
}

#[tokio::test]
async fn alter_changes_only_optimize_and_rebuilds_live_function() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "f").await;
    let before = stored_row(&db, "f").await;

    db.alter_function_optimize("f", TierMode::Never)
        .await
        .expect("alter never");

    let d = db.describe_function("f").await.expect("describe");
    assert_eq!(d.optimize, TierMode::Never);
    assert_eq!(d.tier_report.expect("tier").mode, TierMode::Never);
    let after = stored_row(&db, "f").await;
    assert_eq!(after["optimize"].as_str(), Some("never"));
    assert_eq!(
        without_key(&after, "optimize"),
        without_key(&before, "optimize")
    );
    assert_invokes(&db, "f").await;

    db.alter_function_optimize("f", TierMode::Now)
        .await
        .expect("alter now");
    let d = db.describe_function("f").await.expect("describe");
    assert_eq!(d.optimize, TierMode::Now);
    assert_eq!(d.tier_report.expect("tier").mode, TierMode::Now);
}

#[tokio::test]
async fn altered_optimize_persists_across_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    create_echo(&db, "f").await;
    db.alter_function_optimize("f", TierMode::Never)
        .await
        .expect("alter");

    let db = restart(db, &path).await;

    let d = db.describe_function("f").await.expect("describe");
    assert_eq!(d.optimize, TierMode::Never);
    assert_eq!(d.tier_report.expect("tier").mode, TierMode::Never);
}

#[tokio::test]
async fn non_writer_cannot_alter_but_owner_can() {
    let db = ShamirDb::init_memory().await.expect("init");
    db.create_function_from_wasm_as("f", &echo_wasm(), false, Actor::User(1))
        .await
        .expect("create");
    let before = stored_row(&db, "f").await;

    let err = db
        .alter_function_optimize_as("f", TierMode::Never, Actor::User(2))
        .await
        .expect_err("non-writer must be denied");

    assert!(err.to_string().contains("access denied"), "got: {err}");
    assert_eq!(stored_row(&db, "f").await, before);
    db.alter_function_optimize_as("f", TierMode::Never, Actor::User(1))
        .await
        .expect("owner alters");
    assert_eq!(
        stored_row(&db, "f").await["optimize"].as_str(),
        Some("never")
    );
}

#[tokio::test]
async fn corrupt_function_refuses_alter_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    create_echo(&db, "f").await;
    tamper_row(&db, "f", &echo_variant_wasm()).await;
    let db = restart(db, &path).await;
    let before = stored_row(&db, "f").await;

    let err = db
        .alter_function_optimize("f", TierMode::Never)
        .await
        .expect_err("corrupt function must be refused");

    assert!(err.to_string().contains("corrupt"), "got: {err}");
    assert_eq!(stored_row(&db, "f").await, before);
    assert_eq!(
        stored_row(&db, "f").await["optimize"].as_str(),
        Some("auto")
    );
}

#[tokio::test]
async fn in_session_tampering_is_detected_by_export_then_fails_closed() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "f").await;
    tamper_row(&db, "f", &echo_variant_wasm()).await;

    assert!(db.export_function("f").await.is_err());

    let d = db.describe_function("f").await.expect("describe");
    assert!(
        matches!(d.state, FunctionState::Corrupt { .. }),
        "{:?}",
        d.state
    );
    let err = db
        .invoke_function("f", echo_params())
        .await
        .expect_err("invoke must fail closed");
    assert!(err.to_string().contains("corrupt"), "got: {err}");
    let err = db
        .alter_function_optimize("f", TierMode::Never)
        .await
        .expect_err("alter must be refused");
    assert!(err.to_string().contains("corrupt"), "got: {err}");
}

#[tokio::test]
async fn unloadable_function_refuses_alter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.redb");
    let db = open_fjall(&path).await;
    put_row(&db, "f", &simd_wasm(), Vec::new()).await;
    let db = restart(db, &path).await;

    let err = db
        .alter_function_optimize("f", TierMode::Never)
        .await
        .expect_err("unloadable function must be refused");

    assert!(err.to_string().contains("unavailable"), "got: {err}");
}

#[tokio::test]
async fn alter_unknown_function_is_not_found() {
    let db = ShamirDb::init_memory().await.expect("init");

    let err = db
        .alter_function_optimize("missing", TierMode::Never)
        .await
        .expect_err("unknown name");

    assert!(matches!(err, DbError::NotFound(_)), "got: {err}");
}
