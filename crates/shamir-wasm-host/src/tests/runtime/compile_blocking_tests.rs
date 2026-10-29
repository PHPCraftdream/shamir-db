use super::compile_tests::DOUBLE_SOURCE;
use crate::compile::{manifest_path, resolve_sdk_dir};
use crate::wasm::run_compile_blocking;
use crate::{
    compile_rust_source_blocking, compile_rust_source_blocking_with_options,
    compile_rust_source_with_options, FunctionError, RustCompileOptions, WASM_COMPILE_TIMEOUT,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn sdk_resolution_precedence_and_invalid_explicit() {
    let root = tempfile::tempdir().unwrap();
    let sdk = root.path().join("shamir-sdk");
    std::fs::create_dir_all(sdk.join("src")).unwrap();
    std::fs::write(sdk.join("Cargo.toml"), "").unwrap();
    std::fs::write(sdk.join("src/lib.rs"), "").unwrap();
    for sibling in [
        "shamir-sdk-macros",
        "shamir-query-builder",
        "shamir-query-builder-macros",
        "shamir-query-types",
        "shamir-collections",
        "shamir-types",
    ] {
        std::fs::create_dir_all(root.path().join(sibling)).unwrap();
        std::fs::write(root.path().join(sibling).join("Cargo.toml"), "").unwrap();
    }
    let defaults = RustCompileOptions::default();
    assert_eq!(
        resolve_sdk_dir(&defaults, Some(sdk.clone().into_os_string())).unwrap(),
        sdk.canonicalize().unwrap()
    );
    let invalid_env = root.path().join("invalid-env");
    let error = resolve_sdk_dir(&defaults, Some(invalid_env.into_os_string()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid-env"), "{error}");
    let options = RustCompileOptions {
        sdk_dir: Some(sdk.clone()),
    };
    assert_eq!(
        resolve_sdk_dir(&options, Some(root.path().join("invalid").into_os_string())).unwrap(),
        sdk.canonicalize().unwrap()
    );
    let invalid = RustCompileOptions {
        sdk_dir: Some(root.path().join("invalid")),
    };
    assert!(resolve_sdk_dir(&invalid, Some(sdk.clone().into_os_string())).is_err());
    std::fs::remove_file(root.path().join("shamir-types/Cargo.toml")).unwrap();
    let error = resolve_sdk_dir(&options, None).unwrap_err().to_string();
    assert!(error.contains("shamir-types"), "{error}");
}

fn copy_source_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_source_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn copied_sdk_bundle(root: &Path) -> std::path::PathBuf {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let crates = root.join("crates");
    std::fs::create_dir_all(&crates).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n[workspace.lints.rust]\n[workspace.lints.clippy]\n[workspace.dependencies]\nbench-scale-tool = \"0.1.0\"\n").unwrap();
    for name in [
        "shamir-sdk",
        "shamir-sdk-macros",
        "shamir-query-builder",
        "shamir-query-builder-macros",
        "shamir-query-types",
        "shamir-collections",
        "shamir-types",
    ] {
        copy_source_tree(&checkout.join("crates").join(name), &crates.join(name));
    }
    crates.join("shamir-sdk")
}

#[test]
fn copied_sdk_bundle_compiles_without_checkout() {
    let root = tempfile::tempdir().unwrap();
    let sdk = copied_sdk_bundle(root.path());
    let options = RustCompileOptions { sdk_dir: Some(sdk) };
    let wasm = compile_rust_source_with_options(DOUBLE_SOURCE, WASM_COMPILE_TIMEOUT, &options)
        .expect("copied SDK bundle must compile independently of the checkout");
    assert!(wasm.starts_with(b"\0asm"));
}

#[test]
fn copied_sdk_version_mismatch_is_actionable() {
    let root = tempfile::tempdir().unwrap();
    let sdk = copied_sdk_bundle(root.path());
    let manifest = sdk.join("Cargo.toml");
    let contents = std::fs::read_to_string(&manifest).unwrap();
    let expected = format!("version = \"{}\"", env!("CARGO_PKG_VERSION"));
    assert!(contents.contains(&expected));
    std::fs::write(
        manifest,
        contents.replacen(&expected, "version = \"99.0.0\"", 1),
    )
    .unwrap();
    let error = compile_rust_source_with_options(
        DOUBLE_SOURCE,
        WASM_COMPILE_TIMEOUT,
        &RustCompileOptions { sdk_dir: Some(sdk) },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("expected shamir-sdk version"), "{error}");
    assert!(error.contains("failed to select a version"), "{error}");
    assert!(
        error.contains("shamir-sdk") && error.contains("99.0.0"),
        "{error}"
    );
    assert!(
        error.contains(&format!("={}", env!("CARGO_PKG_VERSION"))),
        "{error}"
    );
}

#[test]
fn sdk_manifest_path_escapes_special_characters() {
    let path = std::path::Path::new("foo\"bar\nxyz");
    assert_eq!(manifest_path(path).unwrap(), "\"foo\\\"bar\\nxyz\"");
}

#[tokio::test]
async fn async_options_reject_missing_explicit_sdk() {
    let options = RustCompileOptions {
        sdk_dir: Some(std::path::PathBuf::from("missing-sdk-directory")),
    };
    let error =
        compile_rust_source_blocking_with_options(DOUBLE_SOURCE, Duration::from_secs(1), options)
            .await
            .unwrap_err();
    assert!(error.to_string().contains("SDK directory"));
}

// ============================================================================
// RV-DB-B40771A1 — compile_rust_source_blocking (spawn_blocking wrapper)
// ============================================================================

/// Shared ticker helper: increments `counter` every 1 ms while `run` is set.
/// Spawned on the current-thread runtime BEFORE the blocking call is awaited;
/// a frozen counter during the await proves the runtime was starved.
async fn ticker_task(run: Arc<AtomicBool>, counter: Arc<AtomicU64>) {
    while run.load(Ordering::Relaxed) {
        counter.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

/// `compile_rust_source_blocking` must offload the (up to 120 s) cargo build
/// via `spawn_blocking` instead of running it inline on the async caller's
/// thread. On a `current_thread` runtime, an inline call would freeze the
/// ticker task at its pre-compile snapshot; the spawn_blocking wrapper lets
/// it keep ticking.
#[tokio::test(flavor = "current_thread")]
async fn rv_db_b40771a1_blocking_compile_does_not_starve_current_thread_runtime() {
    let run = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn(ticker_task(Arc::clone(&run), Arc::clone(&counter)));

    let snapshot = counter.load(Ordering::Relaxed);
    let res = compile_rust_source_blocking(DOUBLE_SOURCE).await;

    match res {
        Ok(_wasm) => {
            assert!(
                counter.load(Ordering::Relaxed) > snapshot,
                "ticker must advance during the blocking compile — the \
                 compile ran inline on the async thread (no spawn_blocking)"
            );
        }
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            run.store(false, Ordering::Relaxed);
            ticker.await.expect("ticker task should terminate");
            eprintln!(
                "SKIP rv_db_b40771a1_blocking_compile_does_not_starve_current_thread_runtime: {msg}"
            );
            return;
        }
        Err(e) => panic!("blocking compile failed: {e}"),
    }

    run.store(false, Ordering::Relaxed);
    ticker.await.expect("ticker task should terminate");
}

/// Toolchain-independent variant: `run_compile_blocking` must execute its
/// closure on the blocking thread pool, not inline on the async caller's
/// thread. A 50 ms `std::thread::sleep` inside the closure would freeze the
/// current-thread runtime if inlined; the ticker must still advance.
#[tokio::test(flavor = "current_thread")]
async fn rv_db_b40771a1_run_compile_blocking_does_not_starve_current_thread_runtime() {
    let run = Arc::new(AtomicBool::new(true));
    let counter = Arc::new(AtomicU64::new(0));
    let ticker = tokio::spawn(ticker_task(Arc::clone(&run), Arc::clone(&counter)));

    let snapshot = counter.load(Ordering::Relaxed);
    let res: Result<u64, FunctionError> = run_compile_blocking(|| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        Ok(42u64)
    })
    .await;

    assert_eq!(res.expect("run_compile_blocking should succeed"), 42);
    assert!(
        counter.load(Ordering::Relaxed) > snapshot,
        "ticker must advance during the blocking closure — the closure ran \
         inline on the async thread (no spawn_blocking)"
    );

    run.store(false, Ordering::Relaxed);
    ticker.await.expect("ticker task should terminate");
}
