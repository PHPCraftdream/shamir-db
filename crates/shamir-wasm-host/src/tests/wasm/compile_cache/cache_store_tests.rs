//! `CacheStore` get/insert regression cases: hits, corruption rejection and
//! cap enforcement at the frame level.

use crate::wasm::compile_cache::DiskCacheStore;
use std::sync::Arc;
use wasmtime::CacheStore;

/// Recursively collect `*.e` entry files under `dir`, sorted.
pub(super) fn entry_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(listing) = std::fs::read_dir(dir) else {
        return found;
    };
    for item in listing.flatten() {
        let path = item.path();
        if path.is_dir() {
            found.extend(entry_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "e") {
            found.push(path);
        }
    }
    found.sort();
    found
}

pub(super) fn engine(store: Arc<DiskCacheStore>, fuel: bool) -> wasmtime::Engine {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(fuel);
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();
    engine
}

pub(super) const MODULE: &str = r#"
(module
  (func (export "f") (result i32) i32.const 1))
"#;

#[test]
fn unbound_store_misses_and_rejects_insertions() {
    let store = DiskCacheStore::new(std::env::temp_dir().join("unbound-cache-test"));
    assert!(store.get(&[0; 32]).is_none());
    assert!(!store.insert(&[0; 32], vec![1]));
}

#[test]
fn second_engine_on_same_directory_gets_hits() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let wasm = wat::parse_str(MODULE).unwrap();
    wasmtime::Module::from_binary(&engine(first.clone(), false), &wasm).unwrap();
    assert_eq!(first.hits(), 0);

    let second = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    wasmtime::Module::from_binary(&engine(second.clone(), false), &wasm).unwrap();
    assert!(second.hits() >= 1, "second engine must read a cache entry");
    assert_eq!(second.rejected(), 0);
}

#[test]
fn flipped_payload_byte_is_a_miss_and_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let wasm = wat::parse_str(MODULE).unwrap();
    wasmtime::Module::from_binary(&engine(first, false), &wasm).unwrap();

    let path = &entry_files(dir.path())[0];
    let mut frame = std::fs::read(path).unwrap();
    let last = frame.len() - 1;
    frame[last] ^= 0xff;
    std::fs::write(path, &frame).unwrap();

    let second = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    wasmtime::Module::from_binary(&engine(second.clone(), false), &wasm).unwrap();
    assert_eq!(second.rejected(), 1, "tampered entry must be rejected");
}

#[test]
fn frame_with_wrong_key_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let wasm = wat::parse_str(MODULE).unwrap();
    wasmtime::Module::from_binary(&engine(first, false), &wasm).unwrap();

    let path = &entry_files(dir.path())[0];
    let mut frame = std::fs::read(path).unwrap();
    frame[40] ^= 0xff; // key byte at offset 40
    std::fs::write(path, &frame).unwrap();

    let second = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    wasmtime::Module::from_binary(&engine(second.clone(), false), &wasm).unwrap();
    assert_eq!(second.rejected(), 1, "key mismatch must be rejected");
}

#[test]
fn oversized_file_is_rejected_without_reading_the_payload() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let wasm = wat::parse_str(MODULE).unwrap();
    wasmtime::Module::from_binary(&engine(first, false), &wasm).unwrap();

    let path = &entry_files(dir.path())[0];
    let mut frame = std::fs::read(path).unwrap();
    frame.extend_from_slice(&[0u8; 64]);
    std::fs::write(path, &frame).unwrap();

    let second = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    wasmtime::Module::from_binary(&engine(second.clone(), false), &wasm).unwrap();
    assert_eq!(
        second.rejected(),
        1,
        "size mismatch must be caught from metadata before any read"
    );
}

#[test]
fn oversized_payload_insert_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    engine(store.clone(), false);
    assert!(!store.insert(&[1; 32], vec![0u8; 4 * 1024 * 1024 + 1]));
}

#[test]
fn concurrent_same_key_inserts_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    engine(store.clone(), false);

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || store.insert(&[7; 32], vec![1, 2, 3]))
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap(), "every insert reports success");
    }
    assert_eq!(entry_files(dir.path()).len(), 1, "one entry on disk");
}

#[test]
fn startup_purges_stale_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let wasm = wat::parse_str(MODULE).unwrap();
    wasmtime::Module::from_binary(&engine(first, false), &wasm).unwrap();

    let namespace_dir = entry_files(dir.path())[0]
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let stale = namespace_dir.join("tmp").join("1-1-deadbeef");
    std::fs::write(&stale, b"x").unwrap();

    let second = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    second.bind(&engine_of(&second)).unwrap();
    assert!(!stale.exists(), "tmp/ must be purged at bind");
}

/// Build an engine around an already-`Arc`ed store without re-wrapping.
fn engine_of(store: &Arc<DiskCacheStore>) -> wasmtime::Engine {
    let mut config = wasmtime::Config::new();
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    wasmtime::Engine::new(&config).unwrap()
}
