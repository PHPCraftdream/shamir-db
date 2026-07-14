use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use once_cell::sync::Lazy;
use scc::TreeIndex;
use shamir_collections::THasher;

/// Global deliver-data cache shared across ALL bridge tasks.
///
/// For `DeliverMode::Records` and `DeliverMode::Keys`, the serialised payload
/// (`Vec<u8>`) is deterministic given (change, commit_version, value) —
/// identical across all subscribers. This cache ensures the payload is built
/// once and shared as `Arc<Vec<u8>>` across N bridges, eliminating redundant
/// msgpack encode + interned-key decode per subscriber.
///
/// Key shape: `(commit_version, db_id, repo_hash, change_index, mode_discriminant)`.
/// **CV-first** so eviction is a `remove_range` over the leading u64, not a
/// full-map scan. Migrated from `DashMap` to `scc::TreeIndex` — see Stage 2
/// of the hidden-O(N) sweep (`docs/dev-artifacts/perf/hidden-on-sweep-stage0.md`).
/// `db_id` is the `Arc<ShamirDb>` pointer address cast to `u64`, uniquely
/// identifying the database instance (prevents cross-instance cache pollution
/// in tests where multiple in-memory DBs share repo names and version ranges).
/// Mode discriminant: 0 = Records, 1 = Keys.
static GLOBAL: Lazy<DeliverCache> = Lazy::new(DeliverCache::new);

type CacheKey = (u64, u64, u64, usize, u8);

pub(crate) struct DeliverCache {
    inner: TreeIndex<CacheKey, Arc<Vec<u8>>>,
    evicted_up_to: AtomicU64,
}

impl DeliverCache {
    fn new() -> Self {
        Self {
            inner: TreeIndex::new(),
            evicted_up_to: AtomicU64::new(0),
        }
    }

    fn repo_hash(repo: &str) -> u64 {
        use std::hash::BuildHasher;
        THasher::default().hash_one(repo)
    }
}

/// Try to get a cached deliver payload.
pub(crate) fn deliver_cache_get(
    db_id: u64,
    repo: &str,
    commit_version: u64,
    change_idx: usize,
    mode: u8,
) -> Option<Arc<Vec<u8>>> {
    let key = (
        commit_version,
        db_id,
        DeliverCache::repo_hash(repo),
        change_idx,
        mode,
    );
    GLOBAL.inner.peek_with(&key, |_, v| Arc::clone(v))
}

/// Insert a deliver payload and return a shared reference.
/// Benign race on duplicate key: TreeIndex's insert-once semantics keep
/// the first writer's bytes; cached payloads are deterministic so either
/// is correct from the user's view.
pub(crate) fn deliver_cache_insert(
    db_id: u64,
    repo: &str,
    commit_version: u64,
    change_idx: usize,
    mode: u8,
    data: Vec<u8>,
) -> Arc<Vec<u8>> {
    let key = (
        commit_version,
        db_id,
        DeliverCache::repo_hash(repo),
        change_idx,
        mode,
    );
    let arc = Arc::new(data);
    let _ = GLOBAL.inner.insert_sync(key, Arc::clone(&arc));
    arc
}

/// Evict entries with `commit_version <= up_to`. O(evicted + log N).
pub(crate) fn deliver_cache_evict_up_to(up_to: u64) {
    let prev = GLOBAL.evicted_up_to.load(Ordering::Relaxed);
    if up_to <= prev {
        return;
    }
    if GLOBAL
        .evicted_up_to
        .compare_exchange(prev, up_to, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        let hi = (up_to, u64::MAX, u64::MAX, usize::MAX, u8::MAX);
        GLOBAL.inner.remove_range_sync(..=hi);
    }
}
