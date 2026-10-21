//! `CacheStore` get/insert regression cases: hits, corruption rejection and
//! cap enforcement at the frame level.
//!
//! Path contract: [`DiskCacheStore::new`] takes the DATA root and derives
//! `<root>/cache/wasm`; every assertion inspects that level.

use crate::wasm::compile_cache::DiskCacheStore;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use wasmtime::CacheStore;

/// Recursively collect `*.e` entry files under `dir`, sorted.
pub(super) fn entry_files(dir: &Path) -> Vec<PathBuf> {
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

/// The single `v1-…` namespace directory under `<root>/cache/wasm`.
pub(super) fn namespace_dir(root: &Path) -> PathBuf {
    let cache_wasm = root.join("cache").join("wasm");
    let mut namespaces: Vec<PathBuf> = std::fs::read_dir(&cache_wasm)
        .unwrap()
        .flatten()
        .map(|item| item.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("v1-"))
        })
        .collect();
    assert_eq!(namespaces.len(), 1, "exactly one namespace after bind");
    namespaces.pop().unwrap()
}

/// Build a raw Wasmtime engine around `store` (data root) and bind it.
pub(super) fn engine(store: &Arc<DiskCacheStore>, fuel: bool) -> wasmtime::Engine {
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

/// Parse a 64-char hex file name back into the 32-byte cache key.
pub(super) fn key_of(path: &Path) -> [u8; 32] {
    let name = path.file_name().unwrap().to_string_lossy();
    let mut key = [0u8; 32];
    for (index, chunk) in name[..64].as_bytes().chunks_exact(2).enumerate() {
        key[index] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
    }
    key
}

/// Rewrite the frame at `path` so its key field is `new_key` while its
/// SHA-256 digest stays valid for the modified content.
fn recraft_frame_with_key(path: &Path, new_key: &[u8; 32]) {
    let mut frame = std::fs::read(path).unwrap();
    frame[40..72].copy_from_slice(new_key);
    let mut digest = Sha256::new();
    digest.update(&frame[..80]);
    digest.update(&frame[112..]);
    frame[80..112].copy_from_slice(&digest.finalize());
    std::fs::write(path, &frame).unwrap();
}

#[test]
fn unbound_store_misses_and_rejects_insertions() {
    let dir = tempfile::tempdir().unwrap();
    let store = DiskCacheStore::new(dir.path().to_path_buf());
    assert!(store.get(&[0; 32]).is_none());
    assert!(!store.insert(&[0; 32], vec![1]));
}

#[test]
fn second_engine_on_same_data_root_gets_hits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wasm = wat::parse_str(MODULE).unwrap();

    // Cold engine (raw store, same contract as production) writes the entry.
    let cold = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&cold, false), &wasm).unwrap();
    assert_eq!(cold.hits(), 0);
    assert_eq!(entry_files(&namespace_dir(&root)).len(), 1);

    // A second engine bound to the same DATA root must be served from disk.
    let warm = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&warm, false), &wasm).unwrap();
    assert!(warm.hits() >= 1, "second engine must read a cache entry");
    assert_eq!(warm.rejected(), 0);
}

#[test]
fn flipped_payload_byte_is_a_miss_and_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wasm = wat::parse_str(MODULE).unwrap();
    let first = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&first, false), &wasm).unwrap();

    let path = &entry_files(&namespace_dir(&root))[0];
    let mut frame = std::fs::read(path).unwrap();
    let last = frame.len() - 1;
    frame[last] ^= 0xff;
    std::fs::write(path, &frame).unwrap();

    let second = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&second.clone(), false), &wasm).unwrap();
    assert_eq!(second.rejected(), 1, "tampered entry must be rejected");
    assert!(
        entry_files(&namespace_dir(&root)).is_empty(),
        "tampered entry file must be removed"
    );
}

#[test]
fn truncated_file_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wasm = wat::parse_str(MODULE).unwrap();
    let first = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&first, false), &wasm).unwrap();

    let path = &entry_files(&namespace_dir(&root))[0];
    let frame = std::fs::read(path).unwrap();
    std::fs::write(path, &frame[..frame.len() - 1]).unwrap();

    let second = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&second.clone(), false), &wasm).unwrap();
    assert_eq!(second.rejected(), 1, "truncated entry must be rejected");
    assert!(entry_files(&namespace_dir(&root)).is_empty());
}

#[test]
fn deleted_file_behind_live_index_returns_miss_within_bounded_time() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wasm = wat::parse_str(MODULE).unwrap();
    let first = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&first, false), &wasm).unwrap();
    let path = entry_files(&namespace_dir(&root)).remove(0);
    let key = key_of(&path);

    let second = Arc::new(DiskCacheStore::new(root.clone()));
    engine(&second, false); // bind indexes the entry
    std::fs::remove_file(&path).unwrap();

    // `get` must return a miss promptly, never deadlock.
    let (sender, receiver) = std::sync::mpsc::channel();
    let store = second.clone();
    let lookup = key;
    std::thread::spawn(move || sender.send(store.get(&lookup)).unwrap());
    let result = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("get must not hang when the indexed file is missing");
    assert!(result.is_none(), "missing file must be a miss");

    // The index entry is gone: the follow-up get is a plain miss with no
    // second rejection (no double-count anomaly).
    assert!(second.get(&key).is_none());
    assert_eq!(second.rejected(), 1);
    assert!(!path.exists());
}

#[test]
fn oversized_file_is_a_miss_without_reading_the_payload() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    engine(&store, false);

    // A real indexed entry, then the file behind it grows beyond the cap.
    let key = [0x2a; 32];
    assert!(store.insert(&key, vec![7u8; 16]));
    let path = &entry_files(&namespace_dir(dir.path()))[0];
    std::fs::write(path, vec![0u8; 112 + 4 * 1024 * 1024 + 1]).unwrap();

    assert!(store.get(&key).is_none(), "oversized file must be a miss");
    assert_eq!(
        store.frames_read(),
        0,
        "size mismatch must be caught from metadata before any read"
    );
    assert_eq!(store.rejected(), 1);
    assert!(!path.exists(), "oversized entry must be invalidated");
}

#[test]
fn valid_digest_with_wrong_key_field_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wasm = wat::parse_str(MODULE).unwrap();
    let first = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&first, false), &wasm).unwrap();

    // Re-craft the frame: digest VALID for its content, key field different.
    let path = entry_files(&namespace_dir(&root)).remove(0);
    let lookup_key = key_of(&path);
    recraft_frame_with_key(&path, &[0xee; 32]);

    let second = Arc::new(DiskCacheStore::new(root.clone()));
    engine(&second, false); // bind indexes it under the lookup key
    assert!(second.get(&lookup_key).is_none(), "key mismatch must miss");
    assert_eq!(second.rejected(), 1);
    assert!(!path.exists(), "wrong-key entry must be removed");
}

#[test]
fn oversized_payload_insert_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    engine(&store, false);
    assert!(!store.insert(&[1; 32], vec![0u8; 4 * 1024 * 1024 + 1]));
    assert!(entry_files(&namespace_dir(dir.path())).is_empty());
}

#[test]
fn concurrent_same_key_inserts_leave_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    engine(&store, false);

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || store.insert(&[7; 32], vec![1, 2, 3]))
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap(), "every insert reports success");
    }
    assert_eq!(entry_files(&namespace_dir(dir.path())).len(), 1);
}

#[test]
fn startup_purges_stale_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let wasm = wat::parse_str(MODULE).unwrap();
    let first = Arc::new(DiskCacheStore::new(root.clone()));
    wasmtime::Module::from_binary(&engine(&first, false), &wasm).unwrap();

    let stale = namespace_dir(&root).join("tmp").join("1-1-deadbeef");
    std::fs::write(&stale, b"x").unwrap();

    let second = Arc::new(DiskCacheStore::new(root.clone()));
    second.bind(&engine(&second, false)).unwrap();
    assert!(!stale.exists(), "tmp/ must be purged at bind");
}

/// Concurrent inserts racing one trim: the shared index accounting must end
/// up exactly equal to the on-directory sum (count and bytes).
#[test]
fn concurrent_inserts_and_trim_keep_index_accounting_equal_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    engine(&store, false);

    let inserters: Vec<_> = (0..8)
        .map(|thread| {
            let store = store.clone();
            std::thread::spawn(move || {
                for step in 0..50u32 {
                    let mut key = [0u8; 32];
                    key[..4].copy_from_slice(&(thread * 1000 + step).to_be_bytes());
                    assert!(store.insert(&key, vec![0u8; 16]));
                }
            })
        })
        .collect();
    let trimmer = {
        let store = store.clone();
        std::thread::spawn(move || store.trim_for_test(4_000, 100))
    };
    for handle in inserters {
        handle.join().unwrap();
    }
    trimmer.join().unwrap();

    let files = entry_files(&namespace_dir(dir.path()));
    let disk_bytes: u64 = files
        .iter()
        .map(|path| std::fs::metadata(path).unwrap().len())
        .sum();
    let (index_bytes, index_count) = store.index_totals();
    assert_eq!(index_count, files.len(), "count must match the directory");
    assert_eq!(index_bytes, disk_bytes, "bytes must match the directory");
}
