use shamir_engine::function::{CreateFunctionOptions, ModuleHash, TierMode};

use super::support::{annotated_wasm, create_echo, stored_row};
use crate::shamir_db::{FunctionSource, FunctionState, ShamirDb};
use crate::DbError;

const DOUBLE_SRC: &str = r#"
use shamir::prelude::*;

#[shamir::function]
pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
"#;

#[tokio::test]
async fn create_then_export_and_describe_report_stored_artifact() {
    let db = ShamirDb::init_memory().await.expect("init");
    let input = annotated_wasm();
    let opts = CreateFunctionOptions {
        optimize: TierMode::Now,
        ..CreateFunctionOptions::default()
    };
    db.create_function_with_opts("f", FunctionSource::Wasm(&input), opts)
        .await
        .expect("create");
    let hex = ModuleHash::of(&input).to_hex();

    let export = db.export_function("f").await.expect("export");
    assert_eq!(export.bytes, input);
    assert_eq!(export.hash, hex);
    assert_eq!(export.source, None);

    let row = stored_row(&db, "f").await;
    assert_eq!(row["module_hash"].as_str(), Some(hex.as_str()));
    assert_eq!(row["abi"].as_i64(), Some(1));
    assert_eq!(row["origin"].as_str(), Some("external"));
    assert_eq!(row["optimize"].as_str(), Some("now"));
    assert!(row.get("wasm_hash").is_none());

    let d = db.describe_function("f").await.expect("describe");
    assert_eq!(d.lang, "wasm");
    assert_eq!(d.origin_claim.as_deref(), Some("external"));
    assert_eq!(d.abi_version, Some(1));
    assert_eq!(d.hash.as_deref(), Some(hex.as_str()));
    assert_eq!(d.byte_size, Some(input.len()));
    assert_eq!(d.optimize, TierMode::Now);
    assert_eq!(d.tier_report.expect("tier report").mode, TierMode::Now);
    assert_eq!(d.state, FunctionState::Loaded);
}

#[tokio::test]
async fn export_then_create_from_exported_bytes_round_trips() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "a").await;
    let first = db.export_function("a").await.expect("export a");

    db.create_function_from_wasm("b", &first.bytes, false)
        .await
        .expect("create b");
    let second = db.export_function("b").await.expect("export b");

    assert_eq!(second.bytes, first.bytes);
    assert_eq!(second.hash, first.hash);
}

#[tokio::test]
async fn plain_module_defaults_abi_one_and_no_origin() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "plain").await;

    let d = db.describe_function("plain").await.expect("describe");
    assert_eq!(d.abi_version, Some(1));
    assert_eq!(d.origin_claim, None);
    assert!(stored_row(&db, "plain").await.get("origin").is_none());
}

#[tokio::test]
async fn rust_source_round_trips_through_export() {
    let db = ShamirDb::init_memory().await.expect("init");
    match db
        .create_function_from_source("double", DOUBLE_SRC, false)
        .await
    {
        Ok(()) => {}
        Err(DbError::Function(msg)) if msg.contains("toolchain unavailable") => {
            eprintln!("SKIP rust_source_round_trips_through_export: {msg}");
            return;
        }
        Err(e) => panic!("compile failed: {e}"),
    }

    let export = db.export_function("double").await.expect("export");
    assert_eq!(export.source.as_deref(), Some(DOUBLE_SRC));
    assert_eq!(
        db.describe_function("double").await.expect("d").lang,
        "rust"
    );

    db.create_function_from_wasm("copy", &export.bytes, false)
        .await
        .expect("create copy");
    let copy = db.export_function("copy").await.expect("export copy");
    assert_eq!(copy.hash, export.hash);
}

#[tokio::test]
async fn oversize_source_rejected_before_toolchain_and_stores_nothing() {
    let db = ShamirDb::init_memory().await.expect("init");
    let limit = db.wasm_engine.admission_limits().max_source_bytes;
    let source = "a".repeat(limit + 1);

    let err = db
        .create_function_from_source("big", &source, false)
        .await
        .expect_err("oversize source must fail");

    let msg = err.to_string();
    assert!(msg.contains("limit"), "got: {msg}");
    assert!(!msg.contains("toolchain"), "got: {msg}");
    assert!(db
        .system_store()
        .load_function("big")
        .await
        .expect("load")
        .is_none());
}

#[tokio::test]
async fn rename_keeps_state_and_old_name_is_gone() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "old").await;

    db.rename_function("old", "new").await.expect("rename");

    let d = db.describe_function("new").await.expect("describe new");
    assert_eq!(d.state, FunctionState::Loaded);
    let err = db
        .describe_function("old")
        .await
        .expect_err("old name must be gone");
    assert!(matches!(err, DbError::NotFound(_)), "got: {err}");
}

#[tokio::test]
async fn drop_removes_function_and_state_entry() {
    let db = ShamirDb::init_memory().await.expect("init");
    create_echo(&db, "gone").await;
    assert!(db.function_state.get("gone").is_some());

    assert!(db.drop_function("gone").await.expect("drop"));

    let err = db
        .describe_function("gone")
        .await
        .expect_err("dropped function");
    assert!(matches!(err, DbError::NotFound(_)), "got: {err}");
    assert!(db.function_state.get("gone").is_none());
}
