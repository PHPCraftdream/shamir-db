use shamir_engine::function::{ModuleHash, TierMode};

use super::support::{annotated_wasm, echo_wasm, simd_wasm};
use crate::access::{Actor, ResourceMeta, ResourcePath};
use crate::shamir_db::{FunctionSource, ShamirDb};

async fn sorted_names(db: &ShamirDb) -> Vec<String> {
    db.list_functions().await.expect("list")
}

async fn assert_no_rows(db: &ShamirDb) {
    assert!(db
        .system_store()
        .load_functions()
        .await
        .expect("load rows")
        .is_empty());
}

#[tokio::test]
async fn check_reports_module_and_leaves_no_row_or_registry_entry() {
    let db = ShamirDb::init_memory().await.expect("init");
    let before = sorted_names(&db).await;
    let wasm = annotated_wasm();

    let report = db
        .check_function(FunctionSource::Wasm(&wasm), TierMode::Auto)
        .await
        .expect("check");

    assert_eq!(report.abi_version, 1);
    assert_eq!(report.origin_claim.as_deref(), Some("external"));
    assert_eq!(report.byte_size, wasm.len());
    assert_eq!(report.hash, ModuleHash::of(&wasm));
    assert_no_rows(&db).await;
    assert_eq!(sorted_names(&db).await, before);
}

#[tokio::test]
async fn check_surfaces_admission_and_contract_failures() {
    let db = ShamirDb::init_memory().await.expect("init");
    let empty = wat::parse_str("(module)").expect("empty module");

    let simd = db
        .check_function(FunctionSource::Wasm(&simd_wasm()), TierMode::Auto)
        .await;
    let contract = db
        .check_function(FunctionSource::Wasm(&empty), TierMode::Auto)
        .await;

    assert!(simd.is_err(), "simd module must be rejected");
    assert!(contract.is_err(), "module without exports must be rejected");
    assert_no_rows(&db).await;
}

#[tokio::test]
async fn check_source_denied_without_compiler_execute_but_wasm_allowed() {
    let db = ShamirDb::init_memory().await.expect("init");
    db.set_resource_meta(
        &ResourcePath::WasmCompiler,
        &ResourceMeta {
            owner: Actor::User(1),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("set meta");

    let err = db
        .check_function_as(
            Actor::User(2),
            FunctionSource::Source("pub fn f() {}"),
            TierMode::Auto,
        )
        .await
        .expect_err("source check must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("access denied") && msg.contains("EXECUTE"),
        "got: {msg}"
    );
    assert!(!msg.contains("toolchain"), "got: {msg}");

    let report = db
        .check_function_as(
            Actor::User(2),
            FunctionSource::Wasm(&echo_wasm()),
            TierMode::Auto,
        )
        .await
        .expect("plain Create holder may check wasm");
    assert_eq!(report.hash, ModuleHash::of(&echo_wasm()));
}

#[tokio::test]
async fn check_rejects_oversize_source_without_toolchain() {
    let db = ShamirDb::init_memory().await.expect("init");
    let limit = db.wasm_engine.admission_limits().max_source_bytes;
    let source = "a".repeat(limit + 1);

    let err = db
        .check_function(FunctionSource::Source(&source), TierMode::Auto)
        .await
        .expect_err("oversize source must fail");

    let msg = err.to_string();
    assert!(msg.contains("limit"), "got: {msg}");
    assert!(!msg.contains("toolchain"), "got: {msg}");
}

#[tokio::test]
async fn check_accepts_every_optimize_mode() {
    let db = ShamirDb::init_memory().await.expect("init");
    let wasm = echo_wasm();
    let expected = ModuleHash::of(&wasm);

    for mode in [TierMode::Never, TierMode::Now] {
        let report = db
            .check_function(FunctionSource::Wasm(&wasm), mode)
            .await
            .expect("check");
        assert_eq!(report.hash, expected);
    }
}
