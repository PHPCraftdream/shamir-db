//! Shared compiled-regex cache — one process-wide `scc::HashMap`, no
//! process-global `Mutex`. Lookups take a SHARED per-bucket lock
//! (`scc::HashMap::read_sync`); inserts/compilation never happen under a
//! lock held across the compile itself — patterns are compiled first, then
//! admitted. Per-bucket synchronization is provided by `scc`.
//!
//! Bounded at [`REGEX_CACHE_MAX`] patterns: admission atomically RESERVES a
//! slot in [`CACHE_LEN`] via a compare-and-swap (`fetch_update`) BEFORE the
//! insert, so concurrent distinct misses cannot all observe a stale
//! count and overshoot the cap; the reservation is rolled back
//! (`fetch_sub`) if the insert loses a race to an identical key. Once full,
//! further patterns are compiled per call and NOT cached — cached entries
//! are never bulk-evicted (the old `Mutex` cache cleared itself on the
//! 257th miss, thrashing every hot pattern).

use crate::registry::ScalarError;
use regex::Regex;
use scc::HashMap as SccHashMap;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

const REGEX_CACHE_MAX: usize = 256;

static CACHE: OnceLock<SccHashMap<String, Regex, THasher>> = OnceLock::new();

/// Approximate entry count — `scc::HashMap::len` is O(N) and lint-banned, so
/// the cache maintains its own mirror counter. Updated only through
/// compare-and-swap reservations (see module docs), making the cap tight.
static CACHE_LEN: AtomicUsize = AtomicUsize::new(0);

fn cache() -> &'static SccHashMap<String, Regex, THasher> {
    CACHE.get_or_init(|| SccHashMap::with_hasher(THasher::default()))
}

/// Compile `pat`, reusing the shared cache. Invalid patterns yield
/// `ScalarError("bad_regex")`.
pub(crate) fn compile(pat: &str) -> Result<Regex, ScalarError> {
    let cache = cache();
    // Shared per-bucket read — no exclusive lock, no global Mutex.
    if let Some(entry) = cache.read_sync(pat, |_, v| v.clone()) {
        return Ok(entry);
    }
    let re = Regex::new(pat).map_err(|_| ScalarError::new("bad_regex"))?;
    // Atomically reserve a slot: concurrent misses cannot all pass the cap
    // check, because each CAS moves the counter by exactly one and fails
    // once it has reached REGEX_CACHE_MAX.
    let reserved = CACHE_LEN
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |len| {
            (len < REGEX_CACHE_MAX).then_some(len + 1)
        })
        .is_ok();
    if reserved && cache.insert_sync(pat.to_string(), re.clone()).is_err() {
        // Lost a race to an identical key that was inserted between our
        // shared-read miss and this insert — roll the reservation back so
        // the counter stays tight.
        CACHE_LEN.fetch_sub(1, Ordering::AcqRel);
    }
    Ok(re)
}

/// Whether `pat` is currently cached (test/inspection hook).
#[cfg(test)]
pub(crate) fn is_cached(pat: &str) -> bool {
    cache().contains_sync(pat)
}
