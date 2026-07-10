//! Posting-cache HIT-path bench — audit `2026-07-06-perf-radical-o-notation`
//! findings §1.5 (and the §3.2 representation question).
//!
//! §1.5 — `IndexManager::lookup_by_index` cache-HIT used to deep-clone the
//! ENTIRE `BTreeSet<RecordId>` on every equality lookup. For a
//! low-cardinality index (`status = 'active'` with 100k postings) every
//! query paid 100k tree-node allocations even though the whole point of
//! the cache was to AVOID re-deriving the posting set. The fix returns
//! `Arc<BTreeSet<RecordId>>` so a cache-HIT is an atomic refcount-bump
//! (O(1)) instead of O(|postings|).
//!
//! # Workloads
//!
//! - `lookup_cache_hit/10k_postings` / `lookup_cache_hit/100k_postings` —
//!   repeated equality lookups against a populated, cached posting set.
//!   The first call primes the cache (MISS → populate); every subsequent
//!   timed call is a HIT. With the §1.5 fix a HIT is O(1) regardless of
//!   |postings|; pre-fix it was O(|postings|) — the ratio between the two
//!   posting-set sizes is the speedup signal (a pre-fix run would scale
//!   linearly with |postings|; the post-fix run is flat).
//!
//! # Honest-reporting note
//!
//! The OLD (pre-fix) code can no longer be re-measured without reverting
//! the source, so there is no literal "before" column from a fresh run on
//! the same binary. Instead the signal that the fix landed is the
//! **flatness** of `ns/op` across the 10k vs 100k posting-set sizes: a
//! deep-clone-per-hit implementation would show ~10× growth between the
//! two; the Arc-refcount implementation shows ~1× (both O(1)). This
//! mirrors task #486/#487's precedent of paired/honest measurement when
//! the baseline cannot be re-run.
//!
//! Run: `cargo bench -p shamir-index --bench posting_cache_hit`
//! (uses the isolated bench target dir per the workspace convention).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_index::legacy::index_definition::IndexDefinition;
use shamir_index::legacy::index_info_item::IndexInfoItem;
use shamir_index::legacy::index_manager::IndexManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// A single indexed field id used by the bench's index definition.
const FIELD_ID: u64 = 1;

/// Builds an `IndexManager` over an in-memory store with a single-field
/// regular index on `FIELD_ID`, then populates `n_postings` records that
/// ALL share the same indexed value (`"active"`). This simulates the
/// audit's low-cardinality example (`status = 'active'`).
///
/// Returns the manager and the lookup values to query (so the bench
/// closure can call `lookup_by_index` repeatedly).
async fn build_manager_with_hot_posting(n_postings: usize) -> (IndexManager, Vec<InnerValue>) {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    // Single-field regular index on FIELD_ID.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![FIELD_ID])]);
    manager.create_index(index_def).await.unwrap();

    // The hot indexed value — every record shares it (low cardinality).
    let hot_value = InnerValue::Str("active".to_string());
    let lookup_values = vec![hot_value.clone()];

    // Batched insert: one `now_micros()` per batch, ascending seq tails.
    let batch_ts = RecordId::now_micros();
    for i in 0..n_postings {
        let record_id = RecordId::from_ts_seq(batch_ts, i as u32);
        let mut map = new_map();
        map.insert(InternerKey::new(FIELD_ID), hot_value.clone());
        let value = InnerValue::Map(map);
        manager.on_record_created(&record_id, &value).await.unwrap();
    }

    (manager, lookup_values)
}

/// Primes the posting cache for the given lookup values (one MISS that
/// populates the cache), so every subsequent `lookup_by_index` in the
/// timed closure is a HIT.
async fn prime_cache(manager: &IndexManager, lookup_values: &[InnerValue]) {
    let _ = manager.lookup_by_index(1001, lookup_values).await.unwrap();
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn main() {
    let mut h = Harness::new("posting_cache_hit", env!("CARGO_MANIFEST_DIR"));
    // One runtime for the whole bench process; leak it so each bench
    // closure can capture a `'static` reference (the runtime lives for the
    // duration of the process anyway, and `Harness` closures are
    // `FnMut + 'static`).
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(rt()));

    for &n_postings in &[10_000usize, 100_000usize] {
        // Setup ONCE (plan 1): build the index, populate `n_postings`
        // records sharing one value, prime the cache. The timed closure
        // only does cache-HIT lookups.
        let (manager, lookup_values) = runtime.block_on(build_manager_with_hot_posting(n_postings));
        runtime.block_on(prime_cache(&manager, &lookup_values));

        let id = format!("lookup_cache_hit/{}k_postings", n_postings / 1_000);
        let manager_ref: &'static IndexManager = Box::leak(Box::new(manager));
        h.bench(&id, move || {
            // Every call is a cache HIT — with the §1.5 fix this is an
            // `Arc::clone` (O(1)) regardless of |postings|. Pre-fix it
            // deep-cloned the whole `BTreeSet` (O(|postings|)).
            let ids: Arc<_> = runtime
                .block_on(manager_ref.lookup_by_index(1001, &lookup_values))
                .unwrap();
            // `len()` forces the `Arc` to be materialised (not DCE'd) but
            // is itself O(1) via the slice's `len` through the deref.
            black_box(ids.len());
        });
    }

    h.run();
}
