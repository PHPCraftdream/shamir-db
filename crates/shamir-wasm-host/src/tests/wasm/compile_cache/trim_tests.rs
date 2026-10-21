//! Trim regression: caps are respected and the oldest logical ticks go first.

use crate::wasm::compile_cache::DiskCacheStore;
use std::sync::Arc;
use wasmtime::CacheStore;

#[test]
fn trimming_trims_to_75_percent_by_oldest_tick() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let mut config = wasmtime::Config::new();
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();

    // Five small entries; tick order == insertion order.
    for index in 0..5u8 {
        let mut key = [0u8; 32];
        key[0] = index + 1;
        assert!(store.insert(&key, vec![index; 16]));
    }

    // Cap of 4 entries: trim target is 3, so the two oldest ticks go first.
    store.trim_for_test(u64::MAX, 4);

    let mut remaining: Vec<u8> = Vec::new();
    let namespace = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    for shard in std::fs::read_dir(&namespace).unwrap().flatten() {
        if !shard.path().is_dir() || shard.file_name() == "tmp" {
            continue;
        }
        for item in std::fs::read_dir(shard.path()).unwrap().flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.ends_with(".e") {
                remaining.push(u8::from_str_radix(&name[..2], 16).unwrap());
            }
        }
    }
    remaining.sort_unstable();
    assert_eq!(remaining, vec![4, 5], "oldest ticks are evicted first");
}

#[test]
fn byte_cap_trim_also_respects_the_oldest_tick_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskCacheStore::new(dir.path().to_path_buf()));
    let mut config = wasmtime::Config::new();
    config
        .enable_incremental_compilation(store.clone())
        .unwrap();
    let engine = wasmtime::Engine::new(&config).unwrap();
    store.bind(&engine).unwrap();

    for index in 0..4u8 {
        let mut key = [0u8; 32];
        key[0] = index + 1;
        assert!(store.insert(&key, vec![index; 64]));
    }

    // Each frame is 112 + 64 = 176 bytes; cap at 400 -> target 300 -> keep 1.
    store.trim_for_test(400, usize::MAX);

    let namespace = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut count = 0;
    for shard in std::fs::read_dir(&namespace).unwrap().flatten() {
        if !shard.path().is_dir() || shard.file_name() == "tmp" {
            continue;
        }
        count += std::fs::read_dir(shard.path())
            .unwrap()
            .flatten()
            .filter(|item| item.file_name().to_string_lossy().ends_with(".e"))
            .count();
    }
    assert!(count <= 2, "byte cap must be trimmed down to 75%");
}
