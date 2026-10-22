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
//!
//! Junction layout rule: a junction test NEVER turns a populated directory
//! into a reparse point. Each junction test builds a FRESH data directory
//! whose `cache` is crate-built and whose `cache/wasm` does not exist yet —
//! it is created AS the junction, pointing at a crate-built, first
//! VERIFIED private target directory. The target's tempdir is held in a
//! local binding that outlives every assertion.

use std::path::Path;
use std::sync::Arc;

use shamir_private_dir::{create_private_dir, verify_private_dir};

use super::cache_store_tests::entry_files;
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

/// Creates `link` AS a junction to `target` (`cmd /c mklink /J`); the link
/// path must not exist yet — a populated directory is never converted into
/// a reparse point. Returns false (skip) only when mklink fails (junction
/// privilege missing on the host).
fn make_junction_or_skip(link: &Path, target: &Path) -> bool {
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
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

/// Positive control on the FRESH layout: with a real, crate-built
/// `cache/wasm` (nothing else changed), a raw-bound store persists an
/// entry file below `cache/wasm`.
fn prove_fresh_layout_persists_entries() {
    let control = tempfile::tempdir().unwrap();
    make_private_cache(control.path());
    let store = Arc::new(DiskCacheStore::new(control.path().to_path_buf()));
    let mut config = wasmtime::Config::new();
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();
    let module = wasmtime::Module::from_binary(&engine, &wat::parse_str(MODULE).unwrap()).unwrap();
    drop(module);
    assert!(
        !entry_files(&control.path().join("cache").join("wasm")).is_empty(),
        "the tamper-free control layout must persist at least one entry file"
    );
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
    // Positive control: the same fresh layout with a real private
    // `cache/wasm` persists entries.
    prove_fresh_layout_persists_entries();

    // FRESH junction layout: `cache` is crate-built; `cache/wasm` starts
    // ABSENT and is created AS a junction to a crate-built, verified
    // target — never a populated directory converted into a reparse point.
    let dir = tempfile::tempdir().unwrap();
    create_private_dir(&dir.path().join("cache")).unwrap();
    // `target_root` is held in this local binding, so the junction target
    // outlives every assertion below (no `mem::forget`).
    let target_root = tempfile::tempdir().unwrap();
    let target = target_root.path().join("private-target");
    create_private_dir(&target).unwrap();
    assert!(
        verify_private_dir(&target).is_ok(),
        "the junction target must verify before the link is created"
    );

    let link = dir.path().join("cache").join("wasm");
    if !make_junction_or_skip(&link, &target) {
        return;
    }

    let store = DiskCacheStore::new(dir.path().to_path_buf());
    let engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "a junctioned cache component must disable bind"
    );
}

/// Shared body of the "disabled" proofs through the PUBLIC constructor:
/// on the tampered fresh layout two engines compile without ever serving
/// a hit or writing a single entry file, while the tamper-free control
/// layout persists entries.
fn prove_public_constructor_disabled(data: &Path) {
    let wasm = wat::parse_str(MODULE).unwrap();
    let first = WasmEngine::with_compile_cache(data.to_path_buf())
        .expect("a distrusted cache must degrade, never fail engine construction");
    first.compile(&wasm).unwrap();
    first.compile(&wasm).unwrap();
    let second = WasmEngine::with_compile_cache(data.to_path_buf())
        .expect("a distrusted cache must degrade, never fail engine construction");
    second.compile(&wasm).unwrap();
    assert_eq!(
        second.cache_hits(),
        0,
        "the second engine must never be served from the distrusted disk tier"
    );
    assert_eq!(
        first.cache_hits(),
        0,
        "the first engine must never be served from the distrusted disk tier"
    );
    // No entry file was written anywhere below `cache/wasm`.
    assert!(
        entry_files(&data.join("cache").join("wasm")).is_empty(),
        "no entry file may be written while the disk tier is disabled"
    );
}

#[test]
fn wider_dacl_degrades_to_an_uncached_engine() {
    // Tamper-free control layout: entries ARE written and served.
    let control = tempfile::tempdir().unwrap();
    make_private_cache(control.path());
    let wasm = wat::parse_str(MODULE).unwrap();
    let control_engine = WasmEngine::with_compile_cache(control.path().to_path_buf())
        .expect("a trusted cache must never fail engine construction");
    control_engine.compile(&wasm).unwrap();
    assert!(
        !entry_files(&control.path().join("cache").join("wasm")).is_empty(),
        "the tamper-free control layout must have written entry files"
    );

    // Tampered FRESH layout: the DACL is widened BEFORE any engine runs,
    // so nothing was ever persisted.
    let dir = tempfile::tempdir().unwrap();
    make_private_cache(dir.path());
    widen_dacl_or_panic(&dir.path().join("cache").join("wasm"));
    prove_public_constructor_disabled(dir.path());
}

#[test]
fn junction_as_cache_wasm_degrades_to_an_uncached_engine() {
    // Tamper-free control layout: entries ARE written and served.
    let control = tempfile::tempdir().unwrap();
    make_private_cache(control.path());
    let wasm = wat::parse_str(MODULE).unwrap();
    let control_engine = WasmEngine::with_compile_cache(control.path().to_path_buf())
        .expect("a trusted cache must never fail engine construction");
    control_engine.compile(&wasm).unwrap();
    assert!(
        !entry_files(&control.path().join("cache").join("wasm")).is_empty(),
        "the tamper-free control layout must have written entry files"
    );

    // FRESH junction layout (see junction_as_cache_wasm_disables_bind):
    // `cache/wasm` is created AS a junction to a verified private target.
    let dir = tempfile::tempdir().unwrap();
    create_private_dir(&dir.path().join("cache")).unwrap();
    let target_root = tempfile::tempdir().unwrap();
    let target = target_root.path().join("private-target");
    create_private_dir(&target).unwrap();
    assert!(
        verify_private_dir(&target).is_ok(),
        "the junction target must verify before the link is created"
    );
    if !make_junction_or_skip(&dir.path().join("cache").join("wasm"), &target) {
        return;
    }
    prove_public_constructor_disabled(dir.path());
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
