//! Cold vs warm parity: an engine with the disk cache must produce the same
//! traps as an uncached engine for fuel exhaustion, out-of-bounds memory and
//! epoch interruption — and the warm run must actually be served from the
//! cache (`hits() >= 1`), so a silently disabled cache cannot pass.

use super::cache_store_tests::{engine, entry_files, namespace_dir, MODULE};
use crate::wasm::compile_cache::DiskCacheStore;
use std::sync::Arc;

const INFINITE_LOOP: &str = r#"
(module
  (func (export "spin") (loop (br 0))))
"#;

const OUT_OF_BOUNDS: &str = r#"
(module
  (memory 1)
  (func (export "oob") (result i32) (i32.load offset=0x7ffff000 (i32.const 0))))
"#;

fn run_with_fuel(engine: &wasmtime::Engine, wat: &str, fuel: u64) -> String {
    let wasm = wat::parse_str(wat).unwrap();
    let module = wasmtime::Module::from_binary(engine, &wasm).unwrap();
    let mut store = wasmtime::Store::new(engine, ());
    store.set_fuel(fuel).unwrap();
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
    let run = instance
        .get_typed_func::<(), ()>(&mut store, "spin")
        .unwrap();
    match run.call(&mut store, ()) {
        Ok(()) => panic!("guest must trap on fuel exhaustion"),
        Err(error) => format!("{error:#}"),
    }
}

fn run_out_of_bounds(engine: &wasmtime::Engine) -> String {
    let wasm = wat::parse_str(OUT_OF_BOUNDS).unwrap();
    let module = wasmtime::Module::from_binary(engine, &wasm).unwrap();
    let mut store = wasmtime::Store::new(engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
    let oob = instance
        .get_typed_func::<(), i32>(&mut store, "oob")
        .unwrap();
    match oob.call(&mut store, ()) {
        Ok(_) => panic!("out-of-bounds load must trap"),
        Err(error) => format!("{error}"),
    }
}

/// Raw epoch-interruption engine: no background ticker is spawned; the test
/// itself calls `engine.increment_epoch()` after arming a one-tick deadline.
fn epoch_engine(store: &Arc<DiskCacheStore>) -> wasmtime::Engine {
    let mut config = wasmtime::Config::new();
    config.epoch_interruption(true);
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();
    engine
}

fn run_epoch(engine: &wasmtime::Engine) -> String {
    let wasm = wat::parse_str(INFINITE_LOOP).unwrap();
    let module = wasmtime::Module::from_binary(engine, &wasm).unwrap();
    let mut store = wasmtime::Store::new(engine, ());
    store.set_epoch_deadline(1);
    engine.increment_epoch();
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
    let spin = instance
        .get_typed_func::<(), ()>(&mut store, "spin")
        .unwrap();
    match spin.call(&mut store, ()) {
        Ok(()) => panic!("looping guest must trap on the epoch deadline"),
        Err(error) => format!("{error:#}"),
    }
}

#[test]
fn cold_and_warm_fuel_exhaustion_traps_are_identical() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cold_store = Arc::new(DiskCacheStore::new(root.clone()));
    let cold_trap = run_with_fuel(&engine(&cold_store, true), INFINITE_LOOP, 1_000);

    let warm_store = Arc::new(DiskCacheStore::new(root.clone()));
    let warm_trap = run_with_fuel(&engine(&warm_store, true), INFINITE_LOOP, 1_000);

    assert_eq!(cold_trap, warm_trap, "cache must not change trap behaviour");
    assert!(warm_trap.to_lowercase().contains("fuel"), "{warm_trap}");
    assert!(
        warm_store.hits() >= 1,
        "warm run must be served from the cache"
    );
}

#[test]
fn cold_and_warm_memory_traps_are_identical() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cold_store = Arc::new(DiskCacheStore::new(root.clone()));
    let cold_trap = run_out_of_bounds(&engine(&cold_store, false));

    let warm_store = Arc::new(DiskCacheStore::new(root.clone()));
    let warm_trap = run_out_of_bounds(&engine(&warm_store, false));

    assert_eq!(cold_trap, warm_trap, "cache must not change trap behaviour");
    assert!(
        warm_store.hits() >= 1,
        "warm run must be served from the cache"
    );
}

#[test]
fn cold_and_warm_epoch_interruption_traps_are_identical() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cold_store = Arc::new(DiskCacheStore::new(root.clone()));
    let cold_trap = run_epoch(&epoch_engine(&cold_store));

    let warm_store = Arc::new(DiskCacheStore::new(root.clone()));
    let warm_trap = run_epoch(&epoch_engine(&warm_store));

    assert_eq!(cold_trap, warm_trap, "cache must not change trap behaviour");
    assert!(
        warm_trap.to_lowercase().contains("interrupt"),
        "{warm_trap}"
    );
    assert!(
        warm_store.hits() >= 1,
        "warm run must be served from the cache"
    );
}

#[test]
fn cached_and_uncached_modules_have_identical_exports() {
    let dir = tempfile::tempdir().unwrap();
    let cached_store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let cached = engine(&cached_store, false);
    let uncached_engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();

    let exports = |engine: &wasmtime::Engine| {
        let wasm = wat::parse_str(MODULE).unwrap();
        let module = wasmtime::Module::from_binary(engine, &wasm).unwrap();
        module
            .exports()
            .map(|export| export.name().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(exports(&cached), exports(&uncached_engine));
    assert!(!entry_files(&namespace_dir(dir.path())).is_empty());
}
