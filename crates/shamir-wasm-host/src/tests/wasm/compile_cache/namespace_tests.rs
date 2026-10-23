//! Namespace, directory-trust and startup-recency regression cases.
//!
//! All assertions inspect the `<data_root>/cache/wasm` level, matching the
//! production path contract.

use super::cache_store_tests::{entry_files, key_of, namespace_dir, remaining_keys};
use crate::wasm::compile_cache::DiskCacheStore;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wasmtime::CacheStore;

fn bind(store: &Arc<DiskCacheStore>, fuel: bool) {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(fuel);
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();
}

fn bind_store(root: &Path, fuel: bool) -> Arc<DiskCacheStore> {
    let store = Arc::new(DiskCacheStore::new(root.to_path_buf()));
    bind(&store, fuel);
    store
}

fn bind_store_with_caps(root: &Path, max_bytes: u64, max_entries: usize) -> Arc<DiskCacheStore> {
    let store = Arc::new(DiskCacheStore::with_caps_for_test(
        root.to_path_buf(),
        max_bytes,
        max_entries,
    ));
    bind(&store, false);
    store
}

/// Write a minimal (never-decoded) entry file with the given key and mtime.
fn write_entry(namespace: &Path, key: &[u8; 32], modified: SystemTime) -> PathBuf {
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = namespace.join(&hex[..2]).join(format!("{hex}.e"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, vec![0u8; 200]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    path
}

/// Seed `count` startup entries (first key byte 1..=count) with increasing
/// mtimes under the namespace of a store bound once to `root`.
fn seed_startup_entries(root: &Path, count: u8) -> PathBuf {
    bind_store(root, false); // creates the namespace directory
    let namespace = namespace_dir(root);
    let base = SystemTime::now() - Duration::from_secs(3600);
    for offset in 0..count {
        write_entry(
            &namespace,
            &[offset + 1; 32],
            base + Duration::from_secs(u64::from(offset) * 10),
        );
    }
    namespace
}

#[test]
fn changing_consume_fuel_selects_a_new_namespace_and_removes_the_old() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Engine WITHOUT fuel: bind creates namespace A.
    bind_store(root, false);
    let old = namespace_dir(root);
    assert!(old
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("v1-")));

    // Engine WITH fuel: different compatibility hash -> namespace B, A purged.
    bind_store(root, true);
    let new = namespace_dir(root);
    assert_ne!(
        old, new,
        "different engine settings must never share entries"
    );
    assert!(!old.exists(), "the old namespace directory must be removed");
}

/// Unix-only: mode bits are the trust anchor there; Windows has its
/// counterparts in `windows_trust_tests`.
#[cfg(unix)]
#[test]
fn group_writable_cache_directory_disables_the_disk_tier() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("cache").join("wasm")).unwrap();
    std::fs::set_permissions(
        root.join("cache").join("wasm"),
        std::fs::Permissions::from_mode(0o777),
    )
    .unwrap();

    let store = DiskCacheStore::new(root.to_path_buf());
    let engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "untrusted dir must disable bind"
    );
}

/// Unix-only: symlink components; Windows has its junction counterpart
/// in `windows_trust_tests`.
#[cfg(unix)]
#[test]
fn symlinked_cache_component_disables_the_disk_tier() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("cache")).unwrap();

    let store = DiskCacheStore::new(dir.path().to_path_buf());
    let engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "symlinked cache component must disable bind"
    );
}

/// Startup entries are ranked by mtime, oldest first. Five seeded entries
/// against an entry cap of four are over the cap at bind: the bind-time trim
/// brings the cache to 75% (three entries) and the three NEWEST survive.
#[test]
fn bind_trims_an_over_cap_cache_keeping_the_newest_startup_entries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let namespace = seed_startup_entries(root, 5);

    let store = bind_store_with_caps(root, u64::MAX, 4);
    assert_eq!(remaining_keys(&namespace), vec![3, 4, 5]);
    let (_, count) = store.index_totals();
    assert_eq!(count, 3, "index accounting follows the bind-time trim");
}

/// Live inserts outrank startup entries. Four startup entries (ticks 0..3)
/// plus two fresh inserts (ticks 4 and 5) against a cap of five: the sixth
/// entry exceeds the cap, the trim keeps three, and they are the two fresh
/// entries and the newest startup entry.
#[test]
fn fresh_inserts_outrank_startup_entries_in_a_trim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let namespace = seed_startup_entries(root, 4);

    let store = bind_store_with_caps(root, u64::MAX, 5);
    assert_eq!(remaining_keys(&namespace), vec![1, 2, 3, 4], "no trim yet");

    assert!(store.insert(&[0xaa; 32], vec![1u8; 16]));
    assert!(store.insert(&[0xbb; 32], vec![1u8; 16]));

    assert_eq!(remaining_keys(&namespace), vec![4, 0xaa, 0xbb]);
    assert!(entry_files(&namespace)
        .iter()
        .any(|path| key_of(path) == [0xaa; 32]));
}
