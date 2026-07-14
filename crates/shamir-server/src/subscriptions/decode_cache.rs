use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use once_cell::sync::Lazy;
use scc::TreeIndex;
use shamir_collections::THasher;
use shamir_db::core::interner::Interner;
use tokio::sync::OnceCell;

/// Global decode cache shared across ALL bridge tasks (all connections, all
/// repos). Keyed by `(repo_hash, commit_version, change_index)`.
///
/// Caches `(Arc<[u8]>, Arc<OnceCell<Interner>>)`:
/// - `Arc<[u8]>` is the raw msgpack-encoded record bytes (zero-decode on
///   cache hit — the `RecordView` lens reads fields on demand directly
///   from these bytes without materialising an `InnerValue` tree).
/// - `Arc<OnceCell<Interner>>` is a shared handle to the table's interner;
///   the cell is guaranteed to be populated before insertion (see
///   `ShamirDb::get_table_interner_cell`), so callers can do
///   `cell.get().unwrap()` synchronously for filter field-path resolution.
///
/// Value decoding (`RecordRef` resolution or `InnerValue` construction)
/// only happens when the event passes the filter and must be delivered,
/// eliminating the decode + alloc on rejected events.
///
/// The cache exploits the fact that `tokio::sync::broadcast` delivers
/// `Arc<ChangelogEvent>` to every subscriber: N bridges receiving the same
/// event each see the same msgpack bytes — the first bridge pays the
/// interner lookup; the rest share the cached result.
static GLOBAL: Lazy<DecodeCache> = Lazy::new(DecodeCache::new);

/// Key shape: `(commit_version, db_id, repo_hash, change_index)`. **CV-first**
/// so eviction is a `remove_range` over the leading u64, not a full-map
/// scan. Migrated from `DashMap` (O(1) get, O(N) eviction) to
/// `scc::TreeIndex` (O(log N) get, O(evicted + log N) eviction) — see
/// Stage 2 of the hidden-O(N) sweep (`docs/perf/hidden-on-sweep-stage0.md`).
///
/// `db_id` (the `ShamirDb` instance pointer) is part of the key to prevent
/// cross-database collisions: distinct databases each own a repo named
/// `"main"` whose commit-version sequences start low and overlap, so a key
/// without `db_id` returns a *different db's* record bytes for the same
/// `(repo, commit_version, change_idx)` — corrupting subscription filter
/// evaluation. Mirrors the `db_id` dimension the deliver cache already keys on.
type CacheKey = (u64, u64, u64, usize);

/// Inner tuple cached in the decode table: raw msgpack bytes + initialized interner cell.
pub(crate) type CachedRecordBytes = (Arc<[u8]>, Arc<OnceCell<Interner>>);

/// Cached value: raw msgpack bytes + initialized interner cell.
pub(crate) type CachedBytes = Arc<Option<CachedRecordBytes>>;

pub(crate) struct DecodeCache {
    inner: TreeIndex<CacheKey, CachedBytes>,
    evicted_up_to: AtomicU64,
}

impl DecodeCache {
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

/// Try to get a cached decode result. `db_id` scopes the lookup to one
/// `ShamirDb` instance — see `CacheKey` for why cross-db keying is required.
pub(crate) fn cache_get(
    db_id: u64,
    repo: &str,
    commit_version: u64,
    change_idx: usize,
) -> Option<CachedBytes> {
    let key = (
        commit_version,
        db_id,
        DecodeCache::repo_hash(repo),
        change_idx,
    );
    GLOBAL.inner.peek_with(&key, |_, v| Arc::clone(v))
}

/// Insert a decode result and return a shared reference.
/// Benign race: if two bridges both insert, the first one wins (TreeIndex
/// is insert-once at a key) — bytes are deterministic so either is correct.
pub(crate) fn cache_insert(
    db_id: u64,
    repo: &str,
    commit_version: u64,
    change_idx: usize,
    value: Option<CachedRecordBytes>,
) -> CachedBytes {
    let key = (
        commit_version,
        db_id,
        DecodeCache::repo_hash(repo),
        change_idx,
    );
    let arc = Arc::new(value);
    // Insert-once semantics on TreeIndex: if the key already exists, the
    // returned Err carries (key, value). The first writer wins; subsequent
    // identical inserts succeed at the user-observable layer because the
    // first one's bytes are deterministic-equal to ours.
    let _ = GLOBAL.inner.insert_sync(key, Arc::clone(&arc));
    arc
}

/// Evict entries with `commit_version <= up_to`. O(evicted + log N): the
/// CV-first key shape makes the eviction a contiguous range over the
/// leading u64. Called periodically by any bridge after advancing its
/// watermark. Best-effort: a single bridge wins the CAS and runs the
/// drain; concurrent losers no-op.
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
        // Inclusive upper bound at the max sentinel within `cv == up_to`
        // — evicts every key whose first component (commit_version) <= up_to,
        // across all db_id / repo_hash / change_idx values.
        let hi = (up_to, u64::MAX, u64::MAX, usize::MAX);
        GLOBAL.inner.remove_range_sync(..=hi);
    }
}
