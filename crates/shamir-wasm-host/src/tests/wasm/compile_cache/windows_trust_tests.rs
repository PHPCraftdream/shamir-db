//! Windows directory-trust counterparts (DEC-3 Q5-c): a widened DACL or a
//! junction in the cache path must disable the disk tier — via the raw
//! `bind` and via the public `with_compile_cache` (logged warn, uncached
//! engine) — never fail engine construction. Unix counterparts live in
//! `namespace_tests`.
//!
//! Oracle discipline: every negative case FIRST proves the same setup is
//! trusted while intact (`cache/wasm` built with `create_private_dir`, a
//! cold engine stores an entry, a second engine gets `cache_hits() > 0`)
//! and only THEN tampers exactly the one property under test and asserts
//! the tier is disabled. icacls needs no privilege on a user-owned temp
//! dir, so an icacls failure fails the test loudly; only junction
//! creation (mklink) may skip the test.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shamir_private_dir::create_private_dir;

use crate::wasm::compile_cache::DiskCacheStore;
use crate::wasm::WasmEngine;

const MODULE: &str = r#"
(module
  (func (export "f") (result i32) i32.const 1))
"#;

/// Builds `<data>/cache` and `<data>/cache/wasm` through the crate (NOT
/// `create_dir_all`), so the trusted state is exactly what production
/// creates.
fn make_private_cache(data: &Path) {
    create_private_dir(&data.join("cache")).unwrap();
    create_private_dir(&data.join("cache").join("wasm")).unwrap();
}

/// Warm baseline on the untouched, crate-built directory: a first engine
/// stores an entry and a second engine on the same data root is served
/// from disk (`cache_hits() > 0`). Panics if the trusted setup is not
/// warm — a negative result would then be meaningless.
fn prove_warm_baseline(data: &Path) {
    let wasm = wat::parse_str(MODULE).unwrap();
    let cold = WasmEngine::with_compile_cache(data.to_path_buf())
        .expect("a trusted cache must never fail engine construction");
    cold.compile(&wasm).unwrap();
    let warm = WasmEngine::with_compile_cache(data.to_path_buf())
        .expect("a trusted cache must never fail engine construction");
    warm.compile(&wasm).unwrap();
    assert!(
        warm.cache_hits() > 0,
        "warm baseline broken: the untampered cache must serve hits"
    );
}

/// Grants Everyone full control on `path` via icacls. This needs no
/// privilege on a user-owned temp directory, so failure is a loud test
/// error, never a skip.
fn widen_dacl_or_panic(path: &Path) {
    let output = std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-1-0:(OI)(CI)F"])
        .output()
        .expect("icacls must be available on Windows");
    assert!(
        output.status.success(),
        "icacls could not widen the DACL of {path:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Replaces `cache/wasm` with a junction to `target` — after removing the
/// crate-created real directory, so the ONLY changed property is the
/// reparse point at `cache/wasm`. Returns false (skip) only when mklink
/// fails (junction privilege missing on the host).
fn replace_cache_wasm_with_junction(data: &Path, target: &Path) -> bool {
    let link = data.join("cache").join("wasm");
    std::fs::remove_dir(&link).expect("removing the crate-created cache/wasm must work");
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(target)
        .output()
        .expect("cmd must be available on Windows");
    if output.status.success() {
        return true;
    }
    eprintln!(
        "skipping: mklink /J failed (privilege missing?): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

/// The tampered-directory target the junction points at: itself built and
/// verified by the crate, so a rejection can only come from the reparse
/// point, never from the target's own trust.
fn verified_junction_target() -> PathBuf {
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("private-target");
    create_private_dir(&target).unwrap();
    // Keep the tempdir alive for the test's lifetime.
    std::mem::forget(outside);
    target
}

#[test]
fn wider_dacl_on_cache_wasm_disables_bind() {
    let dir = tempfile::tempdir().unwrap();
    make_private_cache(dir.path());
    prove_warm_baseline(dir.path());

    // Tamper EXACTLY the DACL of cache/wasm (icacls on that level only).
    widen_dacl_or_panic(&dir.path().join("cache").join("wasm"));

    let store = DiskCacheStore::new(dir.path().to_path_buf());
    let engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "a cache directory with a wider DACL must disable bind"
    );
}

#[test]
fn junction_as_cache_wasm_disables_bind() {
    let dir = tempfile::tempdir().unwrap();
    make_private_cache(dir.path());
    prove_warm_baseline(dir.path());

    // Tamper EXACTLY the reparse-point property of cache/wasm: the target
    // itself is crate-built and verifies, so only the junction check can
    // reject.
    if !replace_cache_wasm_with_junction(dir.path(), &verified_junction_target()) {
        return;
    }

    let store = DiskCacheStore::new(dir.path().to_path_buf());
    let engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "a junctioned cache component must disable bind"
    );
}

#[test]
fn wider_dacl_degrades_to_an_uncached_engine() {
    let dir = tempfile::tempdir().unwrap();
    make_private_cache(dir.path());
    prove_warm_baseline(dir.path());

    // Tamper EXACTLY the DACL of cache/wasm (icacls on that level only).
    widen_dacl_or_panic(&dir.path().join("cache").join("wasm"));

    let engine = WasmEngine::with_compile_cache(dir.path().to_path_buf())
        .expect("a distrusted cache must degrade, never fail engine construction");
    let wasm = wat::parse_str(MODULE).unwrap();
    // TWO compiles: the warm baseline proved hits > 0 on this setup, so
    // hits staying 0 after two compiles means nothing was served from
    // disk (bind failure makes with_compile_cache log-and-continue with
    // an unbound store, which never records hits).
    engine.compile(&wasm).unwrap();
    engine.compile(&wasm).unwrap();
    assert_eq!(
        engine.cache_hits(),
        0,
        "disk tier must be disabled after the DACL widening"
    );
}

#[test]
fn junction_as_cache_wasm_degrades_to_an_uncached_engine() {
    let dir = tempfile::tempdir().unwrap();
    make_private_cache(dir.path());
    prove_warm_baseline(dir.path());

    if !replace_cache_wasm_with_junction(dir.path(), &verified_junction_target()) {
        return;
    }

    let engine = WasmEngine::with_compile_cache(dir.path().to_path_buf())
        .expect("a distrusted cache must degrade, never fail engine construction");
    let wasm = wat::parse_str(MODULE).unwrap();
    engine.compile(&wasm).unwrap();
    engine.compile(&wasm).unwrap();
    assert_eq!(
        engine.cache_hits(),
        0,
        "disk tier must be disabled when cache/wasm is a junction"
    );
}

/// Raw-store companion of the warm baseline: a store bound to the intact
/// crate-built directory persists entries; kept here so the raw negative
/// tests above have their positive oracle in the same file.
#[test]
fn crate_built_cache_binds_and_stores() {
    let dir = tempfile::tempdir().unwrap();
    make_private_cache(dir.path());
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let mut config = wasmtime::Config::new();
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();
    let module = wasmtime::Module::from_binary(&engine, &wat::parse_str(MODULE).unwrap()).unwrap();
    drop(module);
    assert!(
        store.index_totals().1 >= 1,
        "the crate-built cache must have persisted at least one entry"
    );
}
