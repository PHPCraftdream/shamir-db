use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::Lazy;
use shamir_collections::THasher;
use shamir_db::core::interner::Interner;
use shamir_db::types::value::InnerValue;
use tokio::sync::OnceCell;

/// Global decode cache shared across ALL bridge tasks (all connections, all
/// repos). Keyed by `(repo_hash, commit_version, change_index)`.
///
/// Caches `(InnerValue, Arc<OnceCell<Interner>>)` rather than
/// `serde_json::Value`:
/// - `InnerValue` is the msgpack-decoded record (no JSON alloc).
/// - `Arc<OnceCell<Interner>>` is a shared handle to the table's interner;
///   the cell is guaranteed to be populated before insertion (see
///   `ShamirDb::decode_record_value_inner`), so callers can do
///   `cell.get().unwrap()` synchronously for filter field-path resolution.
///
/// JSON conversion (`inner_to_json_value`) only happens when the event passes
/// the filter and must be delivered, eliminating the alloc on rejected events.
///
/// The cache exploits the fact that `tokio::sync::broadcast` delivers
/// `Arc<ChangelogEvent>` to every subscriber: N bridges receiving the same
/// event each decode the same msgpack bytes — the first bridge pays the
/// decode; the rest share the cached result.
static GLOBAL: Lazy<DecodeCache> = Lazy::new(DecodeCache::new);

/// Key: (repo_hash, commit_version, change_index).
type CacheKey = (u64, u64, usize);

/// Cached value: decoded InnerValue + initialized interner cell.
pub(crate) type DecodedInner = Arc<Option<(InnerValue, Arc<OnceCell<Interner>>)>>;

pub(crate) struct DecodeCache {
    inner: DashMap<CacheKey, DecodedInner, THasher>,
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
) -> Option<DecodedInner> {
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
    value: Option<(InnerValue, Arc<OnceCell<Interner>>)>,
) -> DecodedInner {
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
