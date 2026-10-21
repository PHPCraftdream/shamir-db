//! Trim regression: caps are respected, the oldest logical ticks go first,
//! and the 75% target computation cannot overflow.

use super::cache_store_tests::{engine, namespace_dir, remaining_keys};
use crate::wasm::compile_cache::{three_quarters, DiskCacheStore};
use std::path::Path;
use std::sync::Arc;
use wasmtime::CacheStore;

fn bound_store(root: &Path, max_bytes: u64, max_entries: usize) -> Arc<DiskCacheStore> {
    let store = Arc::new(DiskCacheStore::with_caps_for_test(
        root.to_path_buf(),
        max_bytes,
        max_entries,
    ));
    engine(&store, false);
    store
}

/// Insert `count` entries whose first key byte is 1..=count, one after the
/// other (so their recency ticks are strictly increasing).
fn insert_keys(store: &DiskCacheStore, count: u8, value_len: usize) {
    for index in 0..count {
        let mut key = [0u8; 32];
        key[0] = index + 1;
        assert!(store.insert(&key, vec![index; value_len]));
    }
}

/// Entry cap 4: the fifth insert exceeds it, the trim targets 75% = 3 entries
/// and evicts the two oldest ticks, so exactly the three newest keys remain.
#[test]
fn entry_cap_trims_to_target_by_oldest_tick() {
    let dir = tempfile::tempdir().unwrap();
    let store = bound_store(dir.path(), u64::MAX, 4);
    insert_keys(&store, 5, 16);

    assert_eq!(remaining_keys(&namespace_dir(dir.path())), vec![3, 4, 5]);
    assert_eq!(store.index_totals(), (u64::from(3 * (112 + 16)), 3));
}

/// Byte cap 400, frames of 112 + 64 = 176 bytes: the third insert reaches
/// 528 bytes, the trim targets 300 and evicts the two oldest frames (176 is
/// the first total at or below 300), leaving key 3; the fourth insert (352
/// bytes) stays under the cap, so keys 3 and 4 remain.
#[test]
fn byte_cap_trims_to_target_by_oldest_tick() {
    let dir = tempfile::tempdir().unwrap();
    let store = bound_store(dir.path(), 400, usize::MAX);
    insert_keys(&store, 4, 64);

    assert_eq!(remaining_keys(&namespace_dir(dir.path())), vec![3, 4]);
    assert_eq!(store.index_totals(), (2 * 176, 2));
}

/// Maximum caps never trim and never overflow.
#[test]
fn maximum_caps_keep_every_entry() {
    let dir = tempfile::tempdir().unwrap();
    let store = bound_store(dir.path(), u64::MAX, usize::MAX);
    insert_keys(&store, 5, 16);

    assert_eq!(
        remaining_keys(&namespace_dir(dir.path())),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(store.index_totals(), (u64::from(5 * (112 + 16)), 5));
}

/// The 75% target is computed in a wider type: it is exact for small caps and
/// does not overflow for the maximum.
#[test]
fn three_quarters_is_exact_and_overflow_free() {
    assert_eq!(three_quarters(0), 0);
    assert_eq!(three_quarters(1), 0);
    assert_eq!(three_quarters(4), 3);
    assert_eq!(three_quarters(400), 300);
    assert_eq!(
        three_quarters(u64::MAX),
        u64::MAX / 4 * 3 + 3 * (u64::MAX % 4) / 4
    );
}
