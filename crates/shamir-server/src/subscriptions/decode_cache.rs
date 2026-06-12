use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::Lazy;
use shamir_collections::THasher;

/// Global decode cache shared across ALL bridge tasks (all connections, all
/// repos). Keyed by `(commit_version, change_index)` — unique within a
/// repo's changefeed. Different repos may share a `commit_version` number,
/// but the corresponding `change_index` + raw bytes will differ, so a
/// collision produces a semantically wrong cached value. To prevent this,
/// the key includes a cheap repo-name hash discriminant.
///
/// The cache exploits the fact that `tokio::sync::broadcast` delivers
/// `Arc<ChangelogEvent>` to every subscriber: N bridges receiving the same
/// event each need the de-interned JSON of every Put change, but the decode
/// is pure and deterministic — running it once and sharing the result
/// eliminates O(N) redundant msgpack-deserialize + interner-lookup work.
static GLOBAL: Lazy<DecodeCache> = Lazy::new(DecodeCache::new);

/// Key: (repo_hash, commit_version, change_index).
type CacheKey = (u64, u64, usize);

pub(crate) struct DecodeCache {
    inner: DashMap<CacheKey, Arc<Option<serde_json::Value>>, THasher>,
    evicted_up_to: AtomicU64,
}

impl DecodeCache {
    fn new() -> Self {
        Self {
            inner: DashMap::with_hasher(THasher::default()),
            evicted_up_to: AtomicU64::new(0),
        }
    }

    fn repo_hash(repo: &str) -> u64 {
        use std::hash::BuildHasher;
        THasher::default().hash_one(repo)
    }
}

/// Try to get a cached decode result.
pub(crate) fn cache_get(
    repo: &str,
    commit_version: u64,
    change_idx: usize,
) -> Option<Arc<Option<serde_json::Value>>> {
    let key = (DecodeCache::repo_hash(repo), commit_version, change_idx);
    GLOBAL.inner.get(&key).map(|r| Arc::clone(r.value()))
}

/// Insert a decode result and return a shared reference.
/// Benign race: if two bridges both decode and insert, the second
/// overwrites the first with an identical value (decode is deterministic).
pub(crate) fn cache_insert(
    repo: &str,
    commit_version: u64,
    change_idx: usize,
    value: Option<serde_json::Value>,
) -> Arc<Option<serde_json::Value>> {
    let key = (DecodeCache::repo_hash(repo), commit_version, change_idx);
    let arc = Arc::new(value);
    GLOBAL.inner.insert(key, Arc::clone(&arc));
    arc
}

/// Evict entries with `commit_version <= up_to`. Called periodically
/// by any bridge after advancing its watermark. Best-effort: a single
/// bridge wins the CAS and runs `retain`; concurrent losers no-op.
pub(crate) fn cache_evict_up_to(up_to: u64) {
    let prev = GLOBAL.evicted_up_to.load(Ordering::Relaxed);
    if up_to <= prev {
        return;
    }
    if GLOBAL
        .evicted_up_to
        .compare_exchange(prev, up_to, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        GLOBAL.inner.retain(|&(_, cv, _), _| cv > up_to);
    }
}
