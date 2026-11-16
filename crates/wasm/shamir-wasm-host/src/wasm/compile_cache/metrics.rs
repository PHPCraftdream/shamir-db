//! Global compile-cache metrics. Hits and misses are process-wide counters;
//! disk index totals are derived from the set of currently live indices.

use super::entry_index::EntryIndex;
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};

static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);
static INDEXES: LazyLock<ArcSwap<Vec<Weak<EntryIndex>>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

pub struct CompileCacheMetrics {
    pub hits: u64,
    pub misses: u64,
    pub bytes: u64,
    pub entries: u64,
}

pub fn compile_cache_metrics() -> CompileCacheMetrics {
    let indexes = INDEXES.load();
    let (bytes, entries) = indexes.iter().filter_map(Weak::upgrade).fold(
        (0u64, 0u64),
        |(bytes, entries), index| {
            (
                bytes.saturating_add(index.bytes.load(Ordering::Relaxed)),
                entries.saturating_add(
                    u64::try_from(index.count.load(Ordering::Relaxed)).unwrap_or(u64::MAX),
                ),
            )
        },
    );
    CompileCacheMetrics {
        hits: HITS.load(Ordering::Relaxed),
        misses: MISSES.load(Ordering::Relaxed),
        bytes,
        entries,
    }
}

/// Register a disk index. The registry is updated only on the cold bind path;
/// dead store indices are pruned during each update.
pub(crate) fn register_index(index: &Arc<EntryIndex>) {
    INDEXES.rcu(|current| {
        let mut live: Vec<_> = current
            .iter()
            .filter_map(|weak| weak.upgrade().map(|_| weak.clone()))
            .collect();
        live.push(Arc::downgrade(index));
        live
    });
}

pub(crate) fn note_hit() {
    HITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_miss() {
    MISSES.fetch_add(1, Ordering::Relaxed);
}
