//! Trim regression: caps are respected, the oldest logical ticks go first,
//! and `u64::MAX` / `usize::MAX` test hooks do not overflow.

use super::cache_store_tests::{engine, entry_files, namespace_dir};
use crate::wasm::compile_cache::DiskCacheStore;
use std::sync::Arc;
use wasmtime::CacheStore;

fn bound_store(root: &std::path::Path) -> Arc<DiskCacheStore> {
    let store = Arc::new(DiskCacheStore::new(root.to_path_buf()));
    engine(&store, false);
    store
}

fn insert_keys(store: &DiskCacheStore, count: u8, value_len: usize) {
    for index in 0..count {
        let mut key = [0u8; 32];
        key[0] = index + 1;
        assert!(store.insert(&key, vec![index; value_len]));
    }
}

/// Entry cap: 5 entries, cap 4 -> target 4/4*3 = 3 -> evict the two oldest
/// ticks, keeping exactly ticks [4, 5].
#[test]
fn entry_cap_trims_to_target_by_oldest_tick() {
    let dir = tempfile::tempdir().unwrap();
    let store = bound_store(dir.path());
    insert_keys(&store, 5, 16);

    store.trim_for_test(u64::MAX, 4);
    assert_eq!(entry_files(&namespace_dir(dir.path())).len(), 3);
    let (bytes, count) = store.index_totals();
    assert_eq!(count, 3);
    assert_eq!(bytes, u64::from(3 * (112 + 16)));
}

/// Byte cap: 4 frames of 112 + 64 = 176 bytes; cap 400 -> target 300.
/// Evicting the 3 oldest frames leaves 176 <= 300 bytes (1 entry), while
/// evicting only 2 would leave 352 > 300 — so exactly 1 entry remains.
#[test]
fn byte_cap_trims_to_target_by_oldest_tick() {
    let dir = tempfile::tempdir().unwrap();
    let store = bound_store(dir.path());
    insert_keys(&store, 4, 64);

    store.trim_for_test(400, usize::MAX);
    assert_eq!(entry_files(&namespace_dir(dir.path())).len(), 1);
    let (bytes, count) = store.index_totals();
    assert_eq!(count, 1);
    assert!(bytes <= 300, "remaining bytes must be within the target");
}

/// `u64::MAX` / `usize::MAX` hooks must not overflow (target computation is
/// divide-then-multiply): a trim at the maximum caps is a no-op.
#[test]
fn maximum_cap_hooks_do_not_overflow_or_lose_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = bound_store(dir.path());
    insert_keys(&store, 5, 16);

    store.trim_for_test(u64::MAX, usize::MAX);
    assert_eq!(entry_files(&namespace_dir(dir.path())).len(), 5);
    assert_eq!(store.index_totals(), (u64::from(5 * (112 + 16)), 5));
}
