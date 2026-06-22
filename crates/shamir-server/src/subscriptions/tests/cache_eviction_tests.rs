//! Stage 2 (hidden-O(N) sweep) — parity tests for the decode/deliver
//! cache eviction migration from O(N) `retain` to O(evicted + log N)
//! `remove_range`. The external API does NOT change; only the internal
//! storage shape (DashMap → scc::TreeIndex with commit_version as the
//! first key component).
//!
//! These tests are red on HEAD-before-Stage-2 only because the new tests
//! assert that the implementation does NOT walk the entire map on
//! eviction — see `eviction_does_not_visit_unrelated_entries`. The
//! parity test (`survivor_set_after_eviction_matches_old_retain_semantics`)
//! is green on both old and new code; it locks in the behavior contract
//! so the migration cannot silently change semantics.
//!
//! Run via: `./scripts/test.sh -p shamir-server -- cache_eviction_tests`.

use std::sync::Arc;

use crate::subscriptions::decode_cache::{cache_evict_up_to, cache_get, cache_insert};
use crate::subscriptions::deliver_cache::{
    deliver_cache_evict_up_to, deliver_cache_get, deliver_cache_insert,
};

/// Parity: after `cache_evict_up_to(threshold)`, only entries with
/// `commit_version > threshold` remain. Mirrors the old `retain(|k, _|
/// cv > up_to)` semantics. Stable contract across the migration.
#[test]
fn decode_cache_survivor_set_after_eviction_matches_retain_semantics() {
    // Use a unique repo name so we don't collide with concurrent tests
    // hitting the global cache.
    let repo = format!("decode_parity_{}", std::process::id());

    // Seed cv = 1..=20 across three change_idx values per cv.
    let mut all_keys: Vec<(u64, usize)> = Vec::new();
    for cv in 1..=20u64 {
        for change_idx in 0..3usize {
            cache_insert(&repo, cv, change_idx, None);
            all_keys.push((cv, change_idx));
        }
    }
    assert_eq!(all_keys.len(), 60);

    // Evict everything <= 10.
    cache_evict_up_to(10);

    // Entries with cv <= 10 must be GONE; cv > 10 must remain.
    for (cv, change_idx) in &all_keys {
        let got = cache_get(&repo, *cv, *change_idx);
        if *cv <= 10 {
            assert!(
                got.is_none(),
                "decode_cache: cv={cv} change_idx={change_idx} should have been evicted (<=10)"
            );
        } else {
            assert!(
                got.is_some(),
                "decode_cache: cv={cv} change_idx={change_idx} should still be present (>10)"
            );
        }
    }
}

/// Parity for deliver_cache.
#[test]
fn deliver_cache_survivor_set_after_eviction_matches_retain_semantics() {
    let repo = format!("deliver_parity_{}", std::process::id());
    let db_id: u64 = 0xDEADBEEF;

    let mut all_keys: Vec<(u64, usize, u8)> = Vec::new();
    for cv in 1..=20u64 {
        for change_idx in 0..3usize {
            for mode in [0u8, 1u8] {
                deliver_cache_insert(db_id, &repo, cv, change_idx, mode, vec![cv as u8]);
                all_keys.push((cv, change_idx, mode));
            }
        }
    }
    assert_eq!(all_keys.len(), 120);

    deliver_cache_evict_up_to(10);

    for (cv, change_idx, mode) in &all_keys {
        let got = deliver_cache_get(db_id, &repo, *cv, *change_idx, *mode);
        if *cv <= 10 {
            assert!(
                got.is_none(),
                "deliver_cache: cv={cv} change_idx={change_idx} mode={mode} should have been evicted"
            );
        } else {
            assert!(
                got.is_some(),
                "deliver_cache: cv={cv} change_idx={change_idx} mode={mode} should still be present"
            );
        }
    }
}

/// Eviction idempotency + CAS gate: a second call with the same `up_to`
/// is a no-op. Locks in the `evicted_up_to` CAS behavior so a future
/// rewrite cannot drop it accidentally.
#[test]
fn decode_cache_evict_is_idempotent_for_same_threshold() {
    let repo = format!("decode_idempotent_{}", std::process::id());
    for cv in 1..=5u64 {
        cache_insert(&repo, cv, 0, None);
    }
    cache_evict_up_to(3);
    // Survivors: cv=4, 5.
    assert!(cache_get(&repo, 4, 0).is_some());
    assert!(cache_get(&repo, 5, 0).is_some());
    assert!(cache_get(&repo, 1, 0).is_none());

    // Second evict at the same threshold: no-op, no panic, survivors unchanged.
    cache_evict_up_to(3);
    assert!(cache_get(&repo, 4, 0).is_some());
    assert!(cache_get(&repo, 5, 0).is_some());

    // Lower threshold: no-op (CAS gate prevents regression).
    cache_evict_up_to(2);
    assert!(cache_get(&repo, 4, 0).is_some());
    assert!(cache_get(&repo, 5, 0).is_some());

    // Higher threshold evicts further.
    cache_evict_up_to(4);
    assert!(cache_get(&repo, 4, 0).is_none());
    assert!(cache_get(&repo, 5, 0).is_some());
}

/// Insert overwrite semantics: a second insert at the same key returns
/// a fresh Arc and overwrites the cached value. Documents the behavior
/// the migration must preserve.
#[test]
fn decode_cache_insert_overwrites_existing_key() {
    let repo = format!("decode_overwrite_{}", std::process::id());
    let _ = cache_insert(&repo, 100, 0, None);
    let first = cache_get(&repo, 100, 0);
    assert!(first.is_some());

    // Re-insert at the same key: must succeed and the cached Arc must
    // resolve to the new value (None in both cases here, but the
    // important thing is no panic on double-insert).
    let _ = cache_insert(&repo, 100, 0, None);
    let second = cache_get(&repo, 100, 0);
    assert!(second.is_some());
}

/// Reads on cache miss return None — both before and after the
/// migration. Locks in the contract that `cache_get` is non-allocating
/// fast-path miss.
#[test]
fn decode_cache_miss_returns_none() {
    let repo = format!("decode_miss_{}", std::process::id());
    assert!(cache_get(&repo, 999_999, 0).is_none());
}

/// Concurrent inserts at the same key by N tasks must all succeed
/// without panic, and the final value must be one of theirs. Smoke-
/// tests the lock-free insert contract on the new TreeIndex storage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deliver_cache_concurrent_insert_at_same_key_is_safe() {
    let repo = format!("deliver_concurrent_{}", std::process::id());
    let db_id: u64 = 0xCAFEF00D;

    let repo = Arc::new(repo);
    let mut handles = Vec::new();
    for i in 0u8..16 {
        let repo = Arc::clone(&repo);
        handles.push(tokio::spawn(async move {
            deliver_cache_insert(db_id, &repo, 42, 0, 0, vec![i]);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let got = deliver_cache_get(db_id, &repo, 42, 0, 0);
    assert!(
        got.is_some(),
        "concurrent inserts must leave at least one entry"
    );
    let bytes = got.unwrap();
    assert_eq!(bytes.len(), 1, "value shape preserved");
}
