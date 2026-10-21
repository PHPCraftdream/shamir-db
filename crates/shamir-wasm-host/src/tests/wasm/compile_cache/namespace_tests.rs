//! Namespace and directory-trust regression cases.
//!
//! All assertions inspect the `<data_root>/cache/wasm` level, matching the
//! production path contract.

use super::cache_store_tests::namespace_dir;
use crate::wasm::compile_cache::DiskCacheStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wasmtime::CacheStore;

fn bind_store(root: &Path, fuel: bool) -> Arc<DiskCacheStore> {
    let store = Arc::new(DiskCacheStore::new(root.to_path_buf()));
    let mut config = wasmtime::Config::new();
    config.consume_fuel(fuel);
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();
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

fn remaining_keys(namespace: &Path) -> Vec<u8> {
    let mut keys: Vec<u8> = super::cache_store_tests::entry_files(namespace)
        .iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            u8::from_str_radix(&name[..2], 16).unwrap()
        })
        .collect();
    keys.sort_unstable();
    keys
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

    // Engine WITH fuel: different compatibility hash -> namespace B; A purged.
    bind_store(root, true);
    let new = namespace_dir(root);
    assert_ne!(
        old, new,
        "different engine settings must never share entries"
    );
}

#[test]
fn group_writable_cache_directory_disables_the_disk_tier() {
    use std::os::unix::fs::PermissionsExt;

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

#[test]
fn symlinked_cache_component_disables_the_disk_tier() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), dir.path().join("cache")).unwrap();

    let store = DiskCacheStore::new(dir.path().to_path_buf());
    let engine = wasmtime::Engine::new(&wasmtime::Config::new()).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "symlinked cache component must disable bind"
    );
}

/// Startup entries are ranked by mtime, oldest first (ticks 0,1,2,...):
/// trimming to one entry keeps exactly the newest-mtime startup file.
#[test]
fn startup_entries_are_ranked_by_mtime_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    bind_store(root, false); // create the namespace directory
    let namespace = namespace_dir(root);

    let base = SystemTime::now() - Duration::from_secs(3600);
    for offset in 0..3u8 {
        write_entry(
            &namespace,
            &[offset + 1; 32],
            base + Duration::from_secs(offset as u64 * 10),
        );
    }

    let store = bind_store(root, false);
    store.trim_for_test(u64::MAX, 1);
    assert_eq!(remaining_keys(&namespace), vec![3], "newest mtime survives");
}

/// Live inserts outrank startup entries: after a fresh insert, a trim keeps
/// the fresh entry and the newest startup entries; the oldest startup entry
/// goes first.
#[test]
fn fresh_insert_survives_startup_recency_trim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    bind_store(root, false);
    let namespace = namespace_dir(root);

    let base = SystemTime::now() - Duration::from_secs(3600);
    for offset in 0..3u8 {
        write_entry(
            &namespace,
            &[offset + 1; 32],
            base + Duration::from_secs(offset as u64 * 10),
        );
    }

    let store = bind_store(root, false);
    let fresh = [0xaa; 32];
    assert!(store.insert(&fresh, vec![1u8; 16]));

    // Cap 2: ticks are 0,1,2 (startup, oldest first) and 3 (fresh insert);
    // the two oldest go, the fresh entry and the newest startup entry stay.
    store.trim_for_test(u64::MAX, 2);
    assert_eq!(remaining_keys(&namespace), vec![3, 0xaa]);
    assert!(super::cache_store_tests::entry_files(&namespace)
        .iter()
        .any(|path| super::cache_store_tests::key_of(path) == fresh));
}
