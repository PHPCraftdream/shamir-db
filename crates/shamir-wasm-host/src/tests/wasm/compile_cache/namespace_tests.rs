//! Namespace and directory-trust regression cases.

use super::cache_store_tests::{engine, MODULE};
use crate::wasm::compile_cache::DiskCacheStore;
use std::sync::Arc;

fn namespace_dirs(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|item| item.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("v"))
        })
        .collect();
    found.sort();
    found
}

#[test]
fn changing_consume_fuel_selects_a_new_namespace_and_removes_the_old() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let wasm = wat::parse_str(MODULE).unwrap();
    // Engine WITHOUT fuel: bind creates namespace A.
    wasmtime::Module::from_binary(&engine(first.clone(), false), &wasm).unwrap();
    let old_namespaces = namespace_dirs(dir.path());
    assert_eq!(old_namespaces.len(), 1);

    // Engine WITH fuel: different compatibility hash -> namespace B; A is purged.
    let second = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    wasmtime::Module::from_binary(&engine(second, true), &wasm).unwrap();
    let new_namespaces = namespace_dirs(dir.path());
    assert_eq!(new_namespaces.len(), 1, "old namespace must be removed");
    assert_ne!(
        old_namespaces[0], new_namespaces[0],
        "different engine settings must never share entries"
    );
}

#[cfg(unix)]
#[test]
fn group_writable_cache_directory_disables_the_disk_tier() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("cache")).unwrap();
    let wasm_dir = dir.path().join("cache").join("wasm");
    std::fs::create_dir_all(&wasm_dir).unwrap();
    std::fs::set_permissions(&wasm_dir, std::fs::Permissions::from_mode(0o777)).unwrap();

    let store = DiskCacheStore::new(dir.path().join("cache").join("wasm"));
    let mut config = wasmtime::Config::new();
    let engine = wasmtime::Engine::new(&config).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "untrusted dir must disable bind"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_cache_directory_disables_the_disk_tier() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), cache.join("wasm")).unwrap();

    let store = DiskCacheStore::new(cache.join("wasm"));
    let config = wasmtime::Config::new();
    let engine = wasmtime::Engine::new(&config).unwrap();
    assert!(
        store.bind(&engine).is_err(),
        "symlinked dir must disable bind"
    );
}
