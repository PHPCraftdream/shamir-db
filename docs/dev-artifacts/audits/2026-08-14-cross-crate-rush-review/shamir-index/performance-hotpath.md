# shamir-index — Performance & O(x→0)

## Summary

The crate's hot paths show strong, well-documented O(x→0) work in places (batched `Store::transact` in the regular-hash apply path, `Arc<[RecordId]>` posting cache, F-78 streaming backfill, sorted early-stop/first-k lookups, SIMD kernels, SQ8 precomputation, `scc::len()` avoidance with annotated exceptions) — but the optimizations are unevenly distributed across the four index families. The dominant theme violations are: a per-row deep-clone of the entire index-definition set in every write planner (all families), unbatched per-key store round-trips on the sorted/unique apply and index2 drop paths, unbounded posting-list materialization with no top-k bound in the FTS lookup path, and full O(index)-sized RAM spikes in the vector snapshot dump/load. None of these are covered by the existing benches (`posting_cache_hit`, `create_index_streaming`, `sq8_hot_path`, `reader_drain_gate` measure the already-fixed paths).

## Findings

### 1. Per-row deep-clone of the whole index-definition set in every write planner
**File:** `src/base_index/index_info.rs:310-315` (`IndexInfo::iter`), `src/base_index/sorted_index_manager.rs:544-550` (`iter_indexes`); call sites `index_manager.rs:2457/2581/2683`, `index_manager_unique.rs:95/175/218/281/473/504/547/1007/1037/1092`, `sorted_index_manager.rs:1674/1754/1827`
**Severity:** high

**Issue:** `IndexInfo::iter()` clones every `IndexDefinition` on every call (`snap[i].clone()` — one `Vec<IndexInfoItem>` + one `Vec<u64>` per path → heap allocations per def), and `SortedIndexManager::iter_indexes()` deep-clones the entire `Vec<SortedIndexDefinition>` (`(**load_local()).clone()`, ~4 heap `Vec`s per def). These are called from `plan_record_created/updated/deleted` (hash), `plan_record_*_unique` / `validate_unique_for_*` (unique — the validate paths additionally `collect()` into a fresh `Vec` per call), and `plan_record_*` (sorted). Every insert/update/delete on an indexed table pays ~I×(P+1) allocations (I = indexes, P = paths/index) purely to read definitions that change only at DDL time — a direct violation of pillar 3 ("allocation in loops") on the hottest write path in the crate. The RCU snapshot `Arc` is already held by the iterator; only the element clones are wasted.

**Failure scenario:** bulk import of 1M rows into a table with 5 indexes (2 paths each) → ~15M+ avoidable heap allocations that survive nothing, on top of allocator pressure from the postings themselves. The `iter_indexes` doc itself defers to a "future `snapshot()` accessor" that was never landed; `validate_unique_for_create_with_defs` exists precisely to avoid the per-call snapshot on batch paths, but the singular per-row paths still pay it.

**Suggested fix:** Add a borrowing iterator (or return `Arc<Vec<Def>>` / `Arc<[Def]>` from `load_local()`), have the planners iterate by reference; for the unique validators, thread a batch-scope def snapshot the way `validate_unique_for_create_with_defs` already models.

### 2. Sorted/unique apply paths issue one store round-trip per posting — the transact batching landed only for the regular-hash family
**File:** `src/base_index/sorted_index_manager.rs:1847-1864` (`apply_ops`), `src/base_index/index_manager_unique.rs:473-481/504-529/547-554`; contrast `src/base_index/index_manager.rs:2729-2746`
**Severity:** high

**Issue:** `IndexManager::apply_ops` (regular-hash) documents and implements collapsing all `SetPosting`/`RemovePosting` ops into ONE ordered `Store::transact` ("one fsync instead of N per-key writes"). The sibling families never got the same fix: `SortedIndexManager::apply_ops` loops `store.set(...).await` / `store.remove(...).await` per op, and the unique `on_record_*_unique` handlers await `add_unique_entry_by_key`/`remove_unique_entry_by_key` per def. On the non-tx direct CRUD path (`TableManager::insert`/`set`), a row update touching a sorted index produces 2 sequential store round-trips (remove+set), and a table with N indexes pays N sequential round-trips (each potentially its own fsync on durable backends) where the hash family pays 1.

**Failure scenario:** update-heavy workload on a table with a sorted index and 2 unique indexes on a durable backend: 3-5 sequential fsyncs per row write vs 1 for an equivalent hash-only table; write throughput scales O(index_count) in round-trips.

**Suggested fix:** Mirror `IndexManager::apply_ops`: fold the ops into one `Store::transact` (or `set_many`/`remove_many`) batch, preserving op order for last-write-wins.

### 3. FTS lookup materializes every matching posting list and sorts all results — no top-k bound
**File:** `src/fts_ranked_backend.rs:297-380` (`lookup`), `src/fts_backend.rs:202-232`
**Severity:** medium

**Issue:** `FtsRankedBackend::lookup` (a) buffers the FULL posting list of every query token into `Vec<(RecordId, FtsPostingValue)>` (`scan_token_with_values`), (b) for `AndAll` builds a second `Vec<BTreeSet<RecordId>>` (`rid_sets`) and folds pairwise intersections — each fold step allocating a fresh `BTreeSet` — (c) probes `intersection.contains(rid)` (O(log M)) per posting, then (d) collects ALL surviving scores into a `TFxMap` and fully sorts them (`ranked.sort_by`) even though BM25 queries are top-k by construction. `IndexResult::Ranked(Vec)` is unbounded: every matching document is returned. `FtsBackend::lookup` has the same materialize-then-fold shape with `BTreeSet`s.

**Failure scenario:** ranked search for a frequent term (or an `OrAny` of two mid-frequency terms) on a 10M-doc table buffers and sorts tens of millions of entries per query — latency and peak memory are O(matching postings), not O(k), and repeated concurrent queries multiply the transient.

**Suggested fix:** score inside the scan stream and keep a bounded max-heap of size k (the exact pattern `brute_force.rs::push_topk` already implements); for `AndAll`, stream-merge the per-token scans (postings are record-id-ordered within a token) instead of materializing full sets; drop the intermediate `rid_sets` Vec.

### 4. `FtsRankedBackend::plan_update` tokenizes the old record twice
**File:** `src/fts_ranked_backend.rs:193-195`
**Severity:** medium

**Issue:** `let old_set = self.tokenize_set(old)` internally calls `tokenize_with_freq(old)` and throws the frequencies away; two lines later `let (_, old_doc_len) = self.tokenize_with_freq(old)` re-runs the full pipeline on the same record. With the `Full` tokenizer (lowercase + stopword filter + Snowball stemming, one heap allocation per token / per n-gram) this doubles the single most expensive CPU step of the FTS update hot path.

**Failure scenario:** every UPDATE of an FTS-indexed text column pays 2× tokenization of the old document; for long documents under bulk update load this is a constant-factor write-throughput loss that profiling will attribute straight to `plan_update`.

**Suggested fix:** call `tokenize_with_freq(old)` once; derive `old_set` from `freq.keys()` (exactly what `tokenize_set` does internally).

### 5. `IndexExpr::Scalar` evaluation constructs a fresh `Interner` per row
**File:** `src/expr.rs:172` (inside `eval_with_scalars`, Scalar arm)
**Severity:** medium

**Issue:** every evaluation of a scalar-backed functional index builds `Interner::new()` as a scratch interner for the `InnerValue → QueryValue → InnerValue` conversion. This runs on every insert/update/delete planned against such an index (`functional_backend.rs::eval_or_null` → `plan_insert`/`plan_update`/`plan_delete`) — a per-row, per-op construction of the interner's internal map structures, even though the doc comment notes scalar leaves don't need it at all.

**Failure scenario:** a table indexed on a `.trusted_pure()` scalar expression under sustained write load pays interner construction + two owned value conversions per row on the write hot path — pure allocation overhead invisible at the API level.

**Suggested fix:** hoist to a `thread_local!` scratch interner (cleared per use), or construct lazily only on the Map/List arms that actually need interning.

### 6. Vector snapshot dump/load fully materializes the graph and sidecar in RAM
**File:** `src/vector/snapshot.rs:404-435` (`fs::read` both dump files whole), `440-445 + 649-688` (all chunk ops accumulated into one `Vec<KvOp>` held until `store.transact` at 642), `529-549` (sidecar clones every live vector), `788-792` (load reassembles whole files), `884-888` (sidecar fields `.clone()`d again before map insertion)
**Severity:** medium

**Issue:** `dump_snapshot_with_gen` reads both `hnsw_rs` dump files entirely into memory, then re-copies each 1 MiB chunk (`chunk.to_vec()` + bincode re-encode) into a single `ops: Vec<KvOp>` that lives until one final `transact` — peak transient ≈ 2-3× the dump size, plus a full duplicate of every live f32 vector for the sidecar (`for_each_vector(... v.to_vec())`). This runs inside the single-flight background snapshot task after every `VECTOR_SNAPSHOT_DELTA_THRESHOLD` mutations, so the spike recurs periodically in steady state. The load path reassembles the whole files from `get_many` results AND clones the entire sidecar (`sidecar.rid_map.clone()` etc. at 884-888) even though `sidecar` is owned and those fields are never used again — boot-time peak ≈ 2-3× index size.

**Failure scenario:** a 5M-vector dim-128 index (dump on the order of GBs) triggers a background snapshot that buffers the whole dump + chunk ops + sidecar vectors concurrently — an O(index-size) RSS spike on a tokio task, unrelated to query or write load, that can OOM a memory-sized server.

**Suggested fix:** stream the dump files in 1 MiB slices and write chunk batches via repeated `transact`/`set_many`, keeping only the manifest write atomic (the flip already provides atomicity); move the sidecar fields out of the owned `sidecar` (`std::mem::take`) instead of cloning; write sidecar vectors in bounded batches.

### 7. index2 `drop_all` sweeps postings key-by-key; FunctionalBackend buffers the whole index first
**File:** `src/fts_backend.rs:240-252`, `src/fts_ranked_backend.rs:401-413`, `src/functional_backend.rs:115-128 + 294-300`
**Severity:** medium

**Issue:** both FTS backends' `drop_all` do `store.remove(key).await` per posting inside the scan loop — O(N) sequential store round-trips (one fsync each on durable backends) per DROP INDEX. `FunctionalBackend::drop_all` is worse on memory: `scan_postings_by_prefix` first materializes ALL `(key, value)` pairs of the entire index into one `Vec` (unbounded — the value bytes of every posting are buffered), then removes them sequentially. Contrast the base_index family's `sweep_index_postings` (`index_manager.rs:1243-1264`), which collects keys then issues ONE `remove_many`, and `apply_index_ops`' use of `transact`.

**Failure scenario:** DROP INDEX on a 50M-posting FTS index → 50M sequential awaited removes (hours on an fsync-per-op backend), and for the functional family the full posting keyspace held in RAM during the sweep.

**Suggested fix:** batch per scan page: collect the page's keys → `remove_many` → next page (never hold more than one page); this also removes the full-index materialization.

### 8. Vector delta-log replay on restart applies ops one at a time
**File:** `src/vector/snapshot.rs:1250-1266` (`replay_delta`)
**Severity:** medium

**Issue:** each `DeltaOp::Upsert` is replayed via a single `adapter.upsert(rid, &vec).await` — on the HNSW f32 path that is one `spawn_blocking` hop + one single-node `hnsw.insert` per op, all strictly sequential. After a crash with up to `VECTOR_SNAPSHOT_DELTA_THRESHOLD` (default 10k) unabsorbed mutations, restart replay does thousands of isolated single-node inserts where `upsert_batch` (one rayon `parallel_insert` — the primitive `rebuild()` already uses per 1k-row page) would parallelize them.

**Failure scenario:** node restarts after heavy write churn; table-open latency grows linearly with pending delta ops × per-op blocking-pool hop instead of amortizing via batch inserts — the snapshot's whole point (O(load) fast open) degrades toward the full-rebuild path it was meant to avoid.

**Suggested fix:** accumulate ops per chunk (or across all chunks) into a `Vec<(RecordId, Vec<f32>)>` and replay via `adapter.upsert_batch`, applying deletes in their original order relative to upserts (e.g. batch between delete boundaries).

### 9. Posting cache is bounded by entry count, not bytes
**File:** `src/base_index/index_manager.rs:60` (`POSTING_CACHE_CAP = 512`), `2868-2881` (count-only eviction)
**Severity:** medium

**Issue:** the posting cache caps at 512 entries of `Arc<[RecordId]>` with no per-entry or total byte budget. A low-cardinality index (boolean flag, `status` enum) has O(rows/distinct) postings per entry; caching 512 such entries pins `512 × avg_postings × 16` bytes with no upper bound tied to rows. The doc's mitigation ("typical workloads concentrate on a handful of values") is an assumption, not an enforcement — and the workload where the cache helps MOST (low cardinality, repeated equality lookups) is exactly the one where each entry is huge.

**Failure scenario:** 20M-row table, indexed `status` column with 8 values → each cached entry ~40MB (2.5M ids × 16B); a secondary low-cardinality index pushes the cache to hundreds of MB–GBs of pinned RSS that never evicts because 8 ≪ 512 entries.

**Suggested fix:** add a byte budget (Σ `entry.len() × 16` tracked in an `AtomicUsize`, decremented on eviction/invalidation) alongside the entry cap; evict largest-first or arbitrarily once the budget is crossed.

### 10. `HnswAdapter` f32 small-index search clones every vector per query
**File:** `src/vector/hnsw_adapter.rs:2831-2835`
**Severity:** low

**Issue:** the `len() <= BRUTE_FORCE_MAX` (256) exact-search branch clones every stored vector (`pairs.push((*i, v.clone()))` — one heap `Vec<f32>` per element per query) before scoring. The quantized twin `search_quantized_bruteforce` (1805-1827) was already fixed by audit #530 to score inside the `iter_sync` closure with zero per-candidate allocation; the f32 branch kept the clone-per-candidate shape (and starts with `Vec::with_capacity(128)` under a 256-element bound).

**Failure scenario:** small warm index (200 vectors, dim 768) under sustained query load pays 200 heap clones of 3KB each per query — allocation churn exactly where the design intends a cheap deterministic microsecond scan.

**Suggested fix:** mirror the #530 fix: collect `(usize, f32)` scored pairs inside the scan callback (the `deleted`/`rid_map` probes already sit outside `iter_sync` in the two-pass shape).

### 11. `build_index2_backend` hardcodes HNSW parameters and ignores the persisted vector config
**File:** `src/build_backend.rs:52-64`
**Severity:** low

**Issue:** every vector backend built on the reopen path gets `max_elements: 100_000, m: 16, ef_construction: 200, ef_search: 50` regardless of `VectorBackendRef::InProcessHnsw { ef_construct, m }` stored in the descriptor — user tuning silently reverts to defaults after restart, and indexes beyond 100k elements depend on hnsw_rs's internal resize instead of a sized allocation. (Perf-themed aspect: ignored tuning + capacity headroom; the config-bypass itself may also belong to a correctness reviewer.)

**Failure scenario:** a user creates a vector index with `m = 32, ef_construct = 400` for recall; after restart the graph is rebuilt with m=16 defaults — recall and build-time characteristics change with no error or signal.

**Suggested fix:** thread `cfg.backend`'s parameters into `HnswConfig` (falling back to the current constants when `External`), and size `max_elements` from the snapshot sidecar's known element count on restore.

### 12. `IndexRegistry::lease_by_field_and_kind` is an O(N) scan per read dispatch — documented, still open
**File:** `src/registry.rs:644-683` (investigation note at 609-643)
**Severity:** low

**Issue:** every index2 read resolves its backend by iterating all registered backends (`by_id.iter_async`), including a `descriptor()` deref per entry. The in-file #1091 investigation honestly documents why the reverse-index conversion is deferred (the `(kind, field_path)` uniqueness question for fts/functional/btree) and notes counts are typically single digits — so this is acknowledged, bounded debt rather than a hidden cost. Recorded here because it IS a per-read O(N) on the read hot path and the blocking condition (uniqueness decision) is a DDL-layer question away.

**Failure scenario:** a table with dozens of index2 backends (multi-tenant-ish schema) pays a full registry traversal + kind-match per query dispatch.

**Suggested fix:** resolve the `(kind_tag, field_path)` uniqueness question at DDL time (option (a) of the embedded investigation), then add the `by_field_kind` reverse `scc::HashMap` following the proven `by_name` template.

### 13. SQ8 fit runs inline on the threshold-crossing write — O(N) stall on one upsert
**File:** `src/vector/hnsw_adapter.rs:2470-2475` (trigger sites), `1292-1685` (`try_fit_and_rebuild`), catch-up loop `1567-1620`
**Severity:** low

**Issue:** the upsert that crosses `FIT_THRESHOLD` awaits the entire fit inline: O(N) quantizer training, a full `parallel_insert` graph build, and a catch-up loop that repeatedly rescans `vectors` (two full `iter_sync` passes per spin, re-quantizing still-pending entries each iteration) with a spin/backoff wait. It is one-shot and single-flight by design, but the crossing writer absorbs the whole transition as tail latency; the surrounding machinery (snapshot/compaction triggers) already demonstrates the background-task pattern.

**Failure scenario:** p99.9 write latency spikes by the full fit duration (tens of ms+ at N≈256×dim) exactly once per adapter, per compaction cycle — visible as a periodic outlier in write-latency histograms.

**Suggested fix:** keep the crossing upsert on the f32 path (correct pre-fit) and hand the fit to a single-flight `tokio::spawn` (mirroring `trigger_snapshot_check`), letting post-fit upserts flip when `is_fitted` lands.

### 14. `BruteForceAdapter` deep-clones the whole snapshot on every drained write batch
**File:** `src/vector/brute_force.rs:84-93` (publish per drain), `175-182` (`clone_snap`)
**Severity:** low

**Issue:** every publish does a full deep clone of `rids` + `vecs` (N heap `Vec<f32>` clones) + `norms` + `index` — O(N·dim) copy per drained batch. Coalescing amortizes bursts, but a steady one-write-at-a-time stream publishes per write. Grep shows this adapter is used by tests, benches, and `shamir-engine`'s `vector_report.rs` example / `vector_search.rs` bench ground-truth — not by production `build_index2_backend` — hence low severity, but the bench's 10k-element ground-truth inserts are silently O(N²·dim) in setup cost.

**Suggested fix:** store `Arc<Vec<f32>>` per vector (clone the outer Vec of Arcs + index only) or use a persistent/immutable structure so publish is O(1) per unchanged element.

### 15. `apply_index_ops_at_commit` does a linear backend find per provenance group
**File:** `src/write_ops.rs:184-196`
**Severity:** nit

**Issue:** for each distinct `(name_interned, epoch)` group the commit path scans `backends.iter().find(...)` — O(groups × backends) per commit, with typical single-digit counts. Harmless today; would matter only if per-table backend counts grew. A small `TFxMap<u64, &backend>` built once per call removes it.

---

**Coverage note (test/bench claims verified):** the crate's benches honestly measure the already-fixed paths (`posting_cache_hit` — Arc-hit flatness; `create_index_streaming` — F-78 O(batch) peak heap with peak-RSS methodology; `sq8_hot_path`; `reader_drain_gate` with a gate-absent control). None of findings 1-9 have a bench or test exercising their cost; the FTS lookup path, the per-row def-clone, the sorted/unique apply fan-out, and the snapshot dump memory spike are all currently unmeasured.
