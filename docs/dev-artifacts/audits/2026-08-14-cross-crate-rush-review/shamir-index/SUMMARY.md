# shamir-index — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-index/` — the secondary-index engine of S.H.A.M.I.R. DB: regular-hash /
unique / sorted / functional / FTS / ANN-vector index families over the info store, with lock-free
registries and RCU reads, persisted metadata, and a snapshot + delta-log persistence layer for the
HNSW vector family.

Review basis: the seven 2026-08-14 lens files under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`, `performance-hotpath.md`,
`api-wire-protocol.md`, `error-handling-lifecycle.md`, `style-claude-md.md` (79 lens-tagged
findings, here renumbered `X.Y` per lens) — synthesized and deduplicated into distinct defects.
Calibrated for structure/tone/rigor against the exemplar summaries
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`; workspace context from
`SUMMARY.md` ("Per-crate breakdown" and "Per-Crate Health Scorecard" rows). During synthesis, the
highest-impact file:line references were spot-checked against the working tree (functional_backend.rs,
tokenizer.rs, sorted_index_manager.rs, build_backend.rs, vector_backend.rs, bm25.rs) — all verified.
Read-only synthesis — no build/test/lint commands were run; no source file under `crates/` was modified.

Dedup convention: where the same root-cause defect was flagged in multiple lens files, it is listed
**once under its primary lens** with a `Dedup:` note naming the other lenses. `error-handling-lifecycle
#3` bundles two distinct defects (the `__vec_snap__` leak and the unbatched/error-swallowing drop_all
sweep) and is split into 6.3 and 6.4; its #10 is a four-sub-item bundle whose corrupt-FTS-value and
FtsStats-underflow sub-items dedup to 5.10 and 2.8 respectively (6.11 keeps the remaining two).

## Executive summary

The crate's infrastructure is exceptionally well-engineered — reader-drain gates with a worked
SeqCst memory-model proof and loom model, RCU/atomics discipline throughout, exemplary on-disk
versioning in the base_index family — but the seven lenses converge on one dominant theme:
**silent wrong-results and silent persistence loss** in the value-hashing and vector-persistence
paths. Fix first: (1) the functional-index hash that collapses every Dec/Big/Bin value to one
constant (1.1 — guaranteed false positives) and the tokenizer case-fold bug that makes every
properly-capitalized Cyrillic/Greek word unmatchable (1.2); (2) the vector-persistence cluster
where a single corrupt delta chunk (1.3), a torn multi-map snapshot capture (2.1), or a discarded
compaction double-write error (6.2) permanently bakes lost mutations into the live index with no
error signal; (3) the one-line-class `SortedIndexManager::load` swallow (6.1) that turns a
transient store error at open into permanent loss of every sorted-index definition on the next DDL.
The reopen path additionally ignores the persisted vector backend config and has no durable SQ8
carrier (5.1/5.2), silently reverting user tuning and quantization on restart.

---

## 1. correctness-tdd

### 1.1 — high — FunctionalBackend hash collapses every Dec/Big/Bin value to an identical posting hash
- **File:line:** `crates/shamir-index/src/functional_backend.rs:173` (`hash_inner`, `_ => h.write_u8(255)`) — verified during synthesis.
- **Issue:** `hash_inner` explicitly covers Null/Bool/Int/F64/Str/List/Map; every other `InnerValue` variant — `Dec`, `Big`, `Bin` (all real variants, per `shamir-types/src/record_view/scalar_ref.rs` and the crate's own `rust_decimal`/`num-bigint` dev-deps) — falls into the catch-all that writes ONLY the tag byte `0xFF` and no content. Both h1 and h2 passes hash just that constant, so **every distinct Decimal/BigInt/binary value produces the same 128-bit posting hash** — a guaranteed collision by construction, not a probabilistic one.
- **Failure scenario:** `CREATE INDEX … FUNCTIONAL(lower(price))` or a plain `IndexExpr::Field` over a Decimal/BigInt/bytes column (eval returns the raw leaf; `eval_or_null` only collapses *errors* to Null). Insert 1,000 rows with 1,000 distinct decimals; a point query for `price = 10.5` scans the shared hash prefix and returns **all 1,000 rows** regardless of value. Update/delete likewise tombstone/remove the shared key.
- **Fix:** Extend the tag scheme with content-hashing arms for `Dec`/`Big`/`Bin` (mirroring base_index `index_keys.rs`'s exhaustive 11-tag scheme, which already handles them via `hash_inner_value`), or fail closed (`IndexError::TypeMismatch` at plan time / reject at index creation) for unsupported leaf types. Red test: two distinct Dec/Big/Bin values must produce different posting keys.
- **TDD note:** `tests/functional_backend_tests.rs` exercises only Str/Int fields (`make_rec(email, age)`) — the hole is untested.

### 1.2 — high — Whitespace/Full tokenizers never case-fold words whose uppercase letters are all non-ASCII (Russian/Greek FTS broken)
- **File:line:** `crates/shamir-index/src/tokenizer.rs:55-57` (WhitespaceTokenizer) and `tokenizer.rs:277-284` (FullTokenizer); the correct Unicode-aware check exists only in `lowercase_cow` (`tokenizer.rs:93-99`, used by UnicodeTokenizer) — verified during synthesis.
- **Issue:** The borrowed-vs-owned decision is `word.bytes().all(|b| b.is_ascii_lowercase() || !b.is_ascii_alphabetic())`. A word with **no ASCII letters** — e.g. `Москва`, `МОСКВА`, `ΣΟΦΟΣ` — satisfies this predicate (all bytes ≥ 0x80 are "not ASCII alphabetic") and is kept `Cow::Borrowed` **without lowercasing**. The document is indexed with the mixed/uppercase token while a lowercase query (`москва`) tokenizes differently; `token_hash` differs → **no match ever**. In `FullTokenizer` this also feeds the uppercase word to the Snowball stemmer, which expects lowercase input, compounding the mismatch. Every properly-capitalized Russian word (sentence-initial words, all proper nouns) is affected.
- **Failure scenario:** FTS index with `TokenizerKind::Full{language: Russian, ..}`; doc body `"Москва — столица"`; query `"москва"` → 0 hits. Same for Whitespace tokenizer.
- **Fix:** Replace the ASCII-only predicate with a Unicode-aware one (`word.chars().all(|c| !c.is_uppercase())`, exactly `lowercase_cow`'s logic) in both tokenizers. Red test: `FullTokenizer::new(Russian,…).tokenize("Москва")` must equal `tokenize("москва")`; same for `WhitespaceTokenizer`.
- **TDD note:** `tests/tokenizer_tests.rs:37-41` (`unicode_cyrillic`) tests Cyrillic lowercasing **only on UnicodeTokenizer** (the one tokenizer that handles it); all Whitespace/Full tests use ASCII — the suite is vacuous for this bug.

### 1.3 — high — Vector delta-replay failure is warned away, then permanently baked in by the next background snapshot
- **File:line:** `crates/shamir-index/src/vector/vector_backend.rs:683-698` (`restore_on_open`); interacts with `snapshot.rs:1287-1330` (`flip_generation` pruning `0..delta_applied_upto` chunks) — verified during synthesis (the warn-and-continue comment is present as described).
- **Issue:** On a successful snapshot load, `restore_on_open` replays delta chunks `>= manifest.delta_applied_upto`. If `replay_delta` errors (one corrupt/unreadable chunk), the code logs `warn!` and **continues serving the incomplete graph**. The comment claims the mutations are only "missing until the next snapshot" — but the next background snapshot dumps the **in-memory adapter** (still missing those mutations), sets `delta_applied_upto = next_delta_idx`, and `flip_generation` **prunes every absorbed delta chunk**. The committed mutations are now permanently absent from the index with no self-heal path (the data store still has the rows; the index silently disagrees).
- **Failure scenario:** One delta chunk gets a bad byte (the per-chunk crc32 exists precisely to catch this) → open loads base + partial replay → after `VECTOR_SNAPSHOT_DELTA_THRESHOLD` further mutations the flip prunes the unreadable chunk → affected rows vanish from all future vector queries, permanently.
- **Fix:** On replay failure, fall back to full `self.rebuild(data_store).await` (branch-3 semantics — re-derivation from the source of truth) instead of warn-and-continue. Red test: corrupt one delta chunk, restart, assert `rebuild_count() == 1` and that post-snapshot rows are still findable after a forced snapshot flip.

### 1.4 — medium — `FtsRankedBackend::plan_update` emits unguarded BumpFtsStats on empty↔non-empty transitions — permanent doc_count/avg_doc_len drift
- **File:line:** `crates/shamir-index/src/fts_ranked_backend.rs:223-232` (contrast `plan_insert`'s guard at 164-166 and `plan_delete`'s guard at 252-258).
- **Issue:** `plan_insert` returns early when `doc_len == 0` and `plan_delete` wraps its Bump in `if doc_len > 0`, but `plan_update` **unconditionally** pushes `Bump{doc_len: old, sign: -1}` and `Bump{doc_len: new, sign: +1}`. For old-empty→new-non-empty the `-1` decrements a doc that was never counted (undercount, forever); for non-empty→empty the `+1` counts a doc that now has zero postings and can never match (overcount, forever). Both skew `doc_count`/`avg_doc_len` permanently; on a freshly-rebuilt backend where `doc_count == 0`, the spurious decrement wraps to `u64::MAX` (`on_delete`'s `fetch_sub`, see 2.8), making `avg_doc_len ≈ 0` and BM25 norms explode/NaN.
- **Failure scenario:** Table with an FTS index; update a row setting the indexed text field from `""` to `"hello world"`; `doc_count` stays at N instead of N+1 while the doc is queryable; subsequent idf/avgdl computations are wrong for every query.
- **Fix:** Guard each Bump as in insert/delete: emit `-old` only `if old_doc_len > 0`, `+new` only `if new_doc_len > 0`. Red test: insert empty-field doc → update to non-empty → assert `doc_count` increments by exactly 1 (and the reverse direction decrements by exactly 1).
- **TDD note:** `tests/fts_ranked_backend_tests.rs` contains **no `plan_update` test at all**. Related: `rebuild()` (382-399) never zeroes stats before re-deriving, so calling it on a live backend double-counts; the test `rebuild_restores_stats_from_data_store` (214-216) **manually zeroes the counters first**, working around the bug rather than asserting the invariant.

### 1.5 — medium — `lookup_by_index` posting-cache miss→scan→insert race can pin a stale entry past the writer's invalidation
- **File:line:** `crates/shamir-index/src/base_index/index_manager.rs:2847-2882` (miss path + insert); invalidation at `apply_ops`/commit in `2889-2902`.
- **Issue:** A reader that (a) misses the cache, (b) scans the store **before** a concurrent tx commit's `transact`, and (c) inserts its scan result **after** the commit's `invalidate_posting_cache_for_ops` ran, installs a stale `Arc<[RecordId]>` that no later invalidation touches. Every subsequent equality lookup for that key returns the stale set (silently missing newly-committed rows) until the 512-entry cache evicts it. There is no epoch/version re-check between scan and insert.
- **Failure scenario:** Reader A starts `lookup_by_index(k)`; tx B commits a new row posting for `k` and invalidates; A finishes its pre-commit scan and caches the old row list. All `k` lookups miss the new row indefinitely.
- **Fix:** Version the cache (global `AtomicU64` epoch bumped by every invalidate; reader records the epoch before scan and skips the insert if it advanced), or invalidate-after-insert from the writer side by re-probing. A Red test can deterministically interleave via the existing `lookup_pause_hook` seam (park the reader between scan and insert).

### 1.6 — medium — Staged vectors bypass dim validation on the in-tx merge paths (debug panic / silent truncation)
- **File:line:** `crates/shamir-index/src/vector/vector_backend.rs:451-457` (`staged_vector` returns `extract_vec` with no length check); `hnsw_adapter.rs:2948-2956` and `brute_force.rs:308-319` (merge loops call `dist.eval(query, vec)` unguarded); the correct guard exists only in `score_staged_candidates` (`hnsw_adapter.rs:2133-2138`).
- **Issue:** `extract_vec` accepts any all-numeric list regardless of length, so a malformed record (wrong-dim vector field) stages cleanly; at commit it only fails at `upsert` (after the tx was acknowledged as staged), and worse, an in-tx `lookup_tx` merges the staged vector via `ShamirDist::eval` → the SIMD kernels (`simd.rs:105/127` etc.) `debug_assert_eq!(a.len(), b.len())` — **panic in debug/test builds** — and in release compute a silently **truncated** distance (kernels use `a.len().min(b.len())`), returning wrong in-tx top-k. `score_staged_candidates` documents and guards exactly this hazard ("one bad row cannot poison the whole query"); the other two merge paths lack the guard.
- **Failure scenario:** a record with a wrong-dim vector field is staged inside a tx; an in-tx vector lookup merges it → debug panic / release silently-wrong neighbor list for that query.
- **Fix:** Skip wrong-dim staged vectors in both merge loops (mirror `score_staged_candidates`), and/or validate dim in `staged_vector` so bad rows fail at stage time with a typed error.

### 1.7 — low — FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation)
- **File:line:** `crates/shamir-index/src/functional_backend.rs:164-172`.
- **Issue:** `InnerValue::Map(m)` is hashed by iterating `m.iter()`; `TMap` is an IndexMap, so iteration follows **insertion order**. Two logically-equal maps serialized with different key orders (msgpack map order is writer-dependent) produce different posting hashes → false negatives on lookup plus remove/set churn on update. Base_index's `hash_inner_value` (`index_keys.rs:161-173`) deliberately makes Map/Set order-independent (per-element hash XOR) and documents the property; the functional scheme lacks it.
- **Failure scenario:** the logically-equal map `{a:1,b:2}` written by two clients in different insertion order indexes under different posting keys → lookups miss and updates churn.
- **Fix:** Mirror the XOR-of-per-entry-hash scheme (or canonical sort by key id) for Map/Set in `hash_inner`. Red test: two maps with equal content built in opposite insert order must hash identically.

### 1.8 — low — `Dot` metric silently clamps distances to 0 for unnormalized vectors in HNSW (inconsistent with BruteForce)
- **File:line:** `crates/shamir-index/src/vector/hnsw_adapter.rs:150-157` (`(1.0 - dot).max(0.0)`), vs `brute_force.rs:125` (`-dot_product`, exact for any magnitudes); same clamp in `quantized_dist.rs:274-277` and `RescoreCtx::score` (440-443).
- **Issue:** The HNSW path's non-negativity clamp collapses every vector pair with `dot >= 1.0` to distance 0, so for unnormalized (legal) inputs the ordering is destroyed by arbitrary tie-breaking while BruteForce (small indexes, `BRUTE_FORCE_MAX`) returns exact ordering — results silently change character as the index grows past 256. The "callers must normalize" precondition is documented but unenforced (no validation at insert or query).
- **Failure scenario:** an unnormalized Dot-metric index returns correct results while small (brute-force) and arbitrarily re-ordered results once HNSW traversal takes over — with no error anywhere.
- **Fix:** Normalize on insert for Dot (store normalized vectors, or reject non-normalized with a typed error), or return `-dot` with max-heap semantics consistently; at minimum validate and surface a `DimMismatch`-style error instead of clamping.

### 1.9 — low — `FtsStats`: torn (count, sum) reads and a divide-by-zero window in `avg_doc_len`
- **File:line:** `crates/shamir-index/src/bm25.rs:72-78` (`avg_doc_len`), `80-92` (`on_insert` two separate fetch_adds) — verified during synthesis (only a `count == 0` guard exists).
- **Issue:** `on_insert` increments `doc_count` then `sum_doc_len` as two independent Relaxed RMWs; a scoring reader between them observes `count > 0, sum == 0` → `avg_doc_len() == 0` → in `term_score`, `dl / avg_doc_len` is `inf` (or `NaN` when `dl == 0`), producing transiently wrong/NaN BM25 scores under concurrent insert+query. The `count == 0 → 1.0` guard exists but not a `sum == 0 && count > 0` guard.
- **Failure scenario:** a query thread scoring between the two RMWs of a concurrent insert emits `inf`/`NaN` scores for that instant's results.
- **Fix:** Clamp: `if count == 0 || sum == 0 { return 1.0; }` (a posting can only exist when sum ≥ 1, so this never masks a legitimate average), or pack both counters into one atomic. Cheap Red test: interleave `on_insert` between the two RMWs (deterministically via a test seam) and assert a finite score.

### 1.10 — low — Unbounded sorted-range upper bound `prefix || 0xFF×64` excludes values with ≥64 leading 0xFF encoded bytes
- **File:line:** `crates/shamir-index/src/base_index/sorted_index_manager.rs:2627-2632` (`range_bounds`, `None` end arm).
- **Issue:** The "infinite" upper bound is `prefix + [0xFF; 64]`. A physical key whose encoded value begins with 64+ `0xFF` bytes (possible for `Bin`-typed indexed fields — `sort_codec::encode_bytes` never escapes `0xFF`, only `0x00`) compares **greater** than this bound and is silently excluded from `lookup_range(None, None)`, `lookup_min`, `lookup_max`, `lookup_first_k`, `lookup_last_k`. (`entry_count` uses a true prefix scan, so `doctor::verify()` would report the entries the lookups can't see.) Str is safe (UTF-8 cannot contain 0xFF).
- **Failure scenario:** a sorted index over a `Bin` column; one row whose bytes start with 64×`0xFF` disappears from every unbounded/min/max range read while remaining counted — an inconsistency `doctor` flags but queries can't reach.
- **Fix:** Compute the true successor bound (increment the prefix's last byte / extend `0xFF` to the backend's max-key-length contract), or document + enforce a max encoded-value length at index creation.

### 1.11 — low — `apply_index_ops_at_commit` silently drops any non-`BumpFtsStats` in-memory op
- **File:line:** `crates/shamir-index/src/write_ops.rs:160-196`.
- **Issue:** Ops that are neither Set/RemovePosting are collected into `in_memory_ops`, but the grouping loop (170-176) only inserts `BumpFtsStats` variants into `ops_by_backend`; any other future in-memory op variant reaching this path would be silently discarded at commit (while the non-tx `apply_index_ops` applies everything to `backend.apply_in_memory`). Latent today (BumpFtsStats is the only variant) but a one-line drift away from a silent-loss bug.
- **Failure scenario:** a future `IndexWriteOp` in-memory variant added without updating the grouping loop is silently dropped at every commit — data loss with zero signal.
- **Fix:** Handle the residual explicitly (log/error on ungrouped in-memory ops), or route all in-memory ops through the backend that produced them.

---

## 2. concurrency-lockfree

### 2.1 — high — Background vector snapshot dumps the live adapter without quiescing; the multi-map sidecar scan is not atomic across maps (torn capture → permanent zombie graph node)
- **File:line:** `crates/shamir-index/src/vector/vector_backend.rs:891-934` (`run_background_snapshot`), triggered from `trigger_snapshot_check` (`:793-858`); scan site `crates/shamir-index/src/vector/snapshot.rs:522-549` (`dump_snapshot_with_gen`).
- **Issue:** `run_background_snapshot` dumps the LIVE `HnswAdapter` while commit-Phase-5d upserts/deletes continue against it. `dump_snapshot_with_gen` first `file_dump`s the graph (seconds for a large index), then performs four INDEPENDENT `iter_sync` scans (`for_each_rid_map`, `for_each_rid_to_internal`, `for_each_deleted`, `for_each_vector`) at four different instants — no freeze, epoch, or copy makes them one coherent snapshot. The code's own comment (`snapshot.rs:524-528`) justifies this with "the engine will gate them behind a quiesce in #402" — but #402 has landed and the background task it added does NOT quiesce; the assumption is stale. The delta-replay safety net does not close this hole: replay re-applies chunks with index ≥ the HWM captured at trigger time, restoring the *new* state of any mutated rid, but cannot remove a *stale* node the torn capture kept live.
- **Failure scenario:** The snapshot fires exactly when writes are hot (it only triggers after `VECTOR_SNAPSHOT_DELTA_THRESHOLD` = 10k delta ops). A rid-replacing `upsert` (which atomically tombstones `old_internal` in `deleted` and swaps `rid_to_internal` under `entry_sync`, but inserts `rid_map[new_internal]` only later) lands between the `deleted` scan and the `rid_map` scan of the same dump. The persisted sidecar then contains `rid_map[old_internal] = rid` (rid_map entries are never removed) but NOT `deleted[old_internal]`. After restart, `load_snapshot` rebuilds a graph where `old_internal` is a live, unfiltered node resolving to the rid via `rid_map` — the rid surfaces TWICE in every top-k. Delta replay's `upsert(rid)` tombstones only the *current* `rid_to_internal` occupant, never the zombie; it persists until the next compaction. Silent wrong results, no error signal.
- **Fix:** Make the dump a coherent point-in-time capture: (a) quiesce Phase-5d promotes for the duration of the dump (the serialization the stale comment assumed), or (b) copy all maps into owned `Vec`s under a single write-barrier/epoch (e.g. a promote-side seqlock or the existing `snapshot_in_flight` flag consulted by the promote path), or (c) at minimum re-order the scans so `deleted` is scanned LAST (captures the most tombstones) and document the residual window — (c) alone only narrows the window.

### 2.2 — medium — `plan_records_created_batch` bypasses the in-flight-online-build dirty-set capture that every other write path performs
- **File:line:** `crates/shamir-index/src/base_index/index_manager.rs:2512-2543` (batch planner; no `def.state == Building && is_build_in_flight` check), contrasted with the single-row planner at `:2447-2489` and `:2593-2604`; the violated invariant is documented at `:2019-2026`.
- **Issue:** `apply_catchup_batch`'s doc asserts "every other write path routes through plan_record_created/updated/deleted, which correctly captures to the dirty-set while a build is in-flight." That is false for the batched planner: it emits direct `SetPosting` ops for EVERY definition, including a `Building` def whose build is registered in `in_flight_builds`. The engine drives both paths — single-row tx inserts through `plan_record_created`, batch tx inserts (`shamir-engine/src/table/table_manager_tx_ops.rs:809,1016`) and non-tx batch inserts (`table_manager_crud.rs:365`) through `plan_records_created_batch`. Today the divergence is mostly benign because inserts only create new rows (a direct write of a row absent from Phase A's pin is end-state-equivalent to capture-then-replay), but the documented invariant is broken, and the moment a batched update/delete planner is added (mirroring `plan_records_created_unique_batch`'s shape) it will silently skip the stale-posting removal that Phase C's pin-vs-current delta exists to perform — the exact "hidden divergence between two planner paths" bug class this crate's provenance/epoch machinery exists to prevent.
- **Failure scenario:** a future batched update/delete planner reuses the batched shape and silently skips dirty-set capture during an online build → stale postings survive Phase C → permanent wrong results for the affected keys.
- **Fix:** Hoist the `in_flight` check + dirty-set capture into `plan_records_created_batch` (same per-def predicate as `plan_record_created`), or make the single-row planner the only sanctioned path and route the batch wrapper through it. Either way, update the `apply_catchup_batch` doc to state the invariant in a way that cannot silently drift (CLAUDE.md F-1/#1027 prescribes invariant-over-name-list for exactly this reason).

### 2.3 — low — TOCTOU between lock-free `is_build_in_flight` check and Mutex-guarded dirty-set insert can leak an orphan dirty-set entry at Phase D
- **File:line:** `crates/shamir-index/src/base_index/index_manager.rs:1018-1032` (`get_or_create_dirty_set`) vs `:991-995` (`clear_build_in_flight`).
- **Issue:** `get_or_create_dirty_set` checks `is_build_in_flight(name)` (lock-free `scc` read), then takes `dirty_sets`' outer `Mutex` and `entry().or_insert_with(...)` — two steps, not one atomic operation. A writer that passes the check just before Phase D's `clear_build_in_flight` removes both the registry entry and the dirty-set entry will re-create the map entry AFTER the removal. Since the build has finished, no Phase C drain ever runs again for that key: the `Arc<Mutex<BTreeSet<RecordId>>>` (and every RecordId subsequently inserted into it by racing writers) leaks for the manager's lifetime. The check and the insert cannot be made atomic without unifying the registry and the dirty-set under one structure — exactly the migration the field's own TODO (`:303-309`, "convert to `scc::HashMap`") already contemplates.
- **Failure scenario:** a slow writer racing Phase D of an online build resurrects an orphaned dirty-set entry that is never drained — bounded memory leak, repeated per build/writer race.
- **Fix:** When executing the TODO, key the dirty-set as a value inside the same `scc::HashMap` entry as the in-flight marker so presence is atomic. Short of that, have `clear_build_in_flight` swap in a sentinel (or re-check `is_build_in_flight` under the outer `dirty_sets` lock held by the writer) so a late `or_insert_with` cannot resurrect a removed entry.

### 2.4 — low — `lease_by_field_and_kind` is an O(N) full-registry scan on every index2 read dispatch
- **File:line:** `crates/shamir-index/src/registry.rs:644-683`.
- **Issue:** Every index2 query dispatch resolves its backend via `by_id.iter_async`, cloning and comparing `descriptor()` (including `paths`) for each registered backend until a match. This is the hottest read path the crate owns (it runs per query, under a held `ReadGuard` from `reader_gate.enter()` at `:650`), and it is O(number of backends on the table) rather than O(1). The #1091 investigation documented in the method doc is thorough and honestly states both why it is "not urgent at current per-table index counts" and what (the `(kind, field_path)` uniqueness question) blocks the reverse-index conversion — documented debt, not a hidden O(N), but it violates pillar 3's direction and should not become permanent.
- **Failure scenario:** a table with dozens of index2 backends pays a full registry traversal + kind-match per query dispatch, under a held reader-gate guard.
- **Fix:** Resolve the fts/functional/btree `(kind, field_path)` uniqueness question (one-per-key vs multi-id) and land the `by_field_kind` reverse `scc::HashMap` sketched in the doc; until then track the debt explicitly (e.g. in the method doc's header) so it is re-evaluated as per-table index counts grow.
- **Dedup:** also flagged by performance-hotpath #12 (same defect, counted once here).

### 2.5 — low — `BruteForceAdapter::search` runs an O(N·dim) exact scan inline on the async runtime (pillar 2: CPU-bound → `spawn_blocking`)
- **File:line:** `crates/shamir-index/src/vector/brute_force.rs:262-325` (`search`, no `spawn_blocking`); `rebuild`'s per-page `upsert` loop also relies on the actor task's in-line processing.
- **Issue:** Every other CPU-heavy path in this crate (`HnswAdapter` graph traversals and inserts, `Sq8Quantizer::fit`, `file_dump`) is pushed to `spawn_blocking`; `BruteForceAdapter::search` computes an exact distance for every stored vector directly on the tokio worker. The adapter is documented as the baseline/test adapter but is reachable in production via `VectorBackend`'s adapter slot (any non-HNSW adapter), and its cost grows without bound with N — at 100k×128d that is tens of ms of blocked worker per query. Related: `upsert`'s `yield_now()` hack (`:249`) documents that write-then-read visibility is not guaranteed — acceptable for a baseline, worth a doc line if the adapter is ever promoted.
- **Failure scenario:** a production non-HNSW vector index grows large; every query blocks a tokio worker for tens of ms, starving other tasks.
- **Fix:** Wrap the scan loop in `tokio::task::spawn_blocking` (the snapshot `Arc` already moves cleanly), matching `HnswAdapter::search`'s shape.

### 2.6 — nit — `ReaderDrainGate` doc invariant ("Never acquire any other lock while holding a `ReadGuard`") is contradicted in letter by `lookup_by_index`'s DashMap access inside the guard scope
- **File:line:** invariant stated at `crates/shamir-index/src/reader_drain_gate.rs:85-86`; DashMap shard-lock acquisitions inside the guard's scope at `crates/shamir-index/src/base_index/index_manager.rs:2826-2877` (`posting_cache.get`, `.iter().next()`, `.insert`).
- **Issue:** The gate's placement invariant is stated absolutely, but the sole production read chokepoint takes DashMap shard locks while holding the `ReadGuard`. Safe in spirit — DashMap shard locks are leaf locks, never held across `.await`, never held while acquiring the gate, so no lock-order cycle is constructible (and `sweep_index_postings` drain runs only after `wait_for_drain` completes) — but the absolute wording invites a future contributor to either "fix" the cache probe (pointless churn) or treat the invariant as aspirational and acquire a real ordering hazard. Doc/code drift on a load-bearing concurrency contract is itself a hazard in this codebase's style.
- **Failure scenario:** a contributor either churns the hot cache probe to "comply", or cites the doc as aspirational when introducing a genuine lock-order violation.
- **Fix:** Refine the invariant's wording to what is actually proved ("the guard must remain the innermost lock in the DDL lock hierarchy — no lock that waits on the gate or any DDL/admission lock may be acquired while holding a `ReadGuard`"), plus a comment at the `posting_cache` probe noting it satisfies this.

### 2.7 — nit — `BruteForceAdapter::join: std::sync::Mutex<Option<JoinHandle>>` lacks the inline contention-model comment CLAUDE.md requires per instance
- **File:line:** `crates/shamir-index/src/vector/brute_force.rs:64` (field), `:129-135` (`shutdown`).
- **Issue:** The lock fits the sanctioned setup/teardown fallback class (locked exactly once, in `shutdown`, never on a hot path, never across `.await`), but CLAUDE.md's F-9/#1076 revision makes the inline contention-model comment the enforcement mechanism for every `std::sync::Mutex` on a runtime struct — precedent from another site is explicitly not sufficient. Every other Mutex in this crate carries the comment; this one is the outlier.
- **Failure scenario:** none at runtime; the audit trail for the sanctioned-exception policy has a hole.
- **Fix:** Add the one-line comment ("one-shot shutdown join-handle slot; locked once at teardown, contention nil").

### 2.8 — nit — `FtsStats::on_delete` uses bare `fetch_sub` — underflow wraps to a huge `doc_count` with no saturating guard
- **File:line:** `crates/shamir-index/src/bm25.rs:87-92` (paired with `:80-85`) — verified during synthesis.
- **Issue:** `doc_count`/`sum_doc_len` are two independent `AtomicU64`s updated with non-atomic pairs (fine for a derived BM25 average, which is approximate by design), but `on_delete`'s `fetch_sub` wraps silently on underflow. Any accounting bug that applies a `BumpFtsStats{sign: -1}` twice for one document (the double-count class `apply_index_ops_at_commit`'s provenance grouping exists to prevent — see 1.4 for a live source of spurious `-1`s) flips `doc_count` to ~2^64, and every subsequent `idf`/`avg_doc_len` becomes garbage with no error signal. The sibling `HnswAdapter::live_count` (`hnsw_adapter.rs:659-673`) already demonstrates the `saturating_sub` discipline for exactly this reason.
- **Failure scenario:** one double-applied delete-Bump silently poisons all subsequent BM25 scoring on the backend.
- **Fix:** `fetch_update` with `saturating_sub` semantics (or clamp at 0) in `on_delete`, mirroring `live_count`'s documented stance that transient underflow must degrade to 0, never wrap.
- **Dedup:** also flagged by error-handling-lifecycle #10 (sub-item d, counted once here).

---

## 3. security-crypto

No auth/HMAC/TLS surface lives in this crate; its crypto-boundary exposure is hash-based key derivation over **untrusted record values and FTS token streams**, decoding of **persisted, potentially tampered blobs**, one `unsafe` SIMD module, and one secret-bearing config field. The crate is unusually disciplined on input clamping and CRC-on-chunks, but systematically relies on the **non-keyed `FxHasher`** for adversarially controlled inputs — directly contradicting CLAUDE.md pillar 4's own justification ("we don't accept untrusted hash inputs here"), since document text and record values *are* untrusted.

### 3.1 — medium — Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints are enforced on hash alone
- **File:line:** `crates/shamir-index/src/base_index/index_keys.rs:186-240` (`compute_leaf_hashes` / `compute_lookup_hashes`), `crates/shamir-index/src/base_index/index_record_key.rs:25-29,62-81`, consumed at `index_manager.rs:2789-2883` (`lookup_by_index`) and `index_manager_unique.rs:350-421` (`check_unique_key`).
- **Issue:** The 25-byte posting key is `hash1 || hash2`, where h1 and h2 are both `rustc_hash::FxHasher` — the *same* non-keyed, non-cryptographic multiply-xor algorithm, one instance merely pre-seeded with the public constant `0x9E3779B97F4A7C15`. The doc calls this "collision resistance", but the two streams are correlated and FxHash's structure makes simultaneous multi-collisions cheap to construct offline (far below the 2^128 brute-force bound; the seed is fixed and public). All hashed material (record field values, composite tuples) is client-controlled row data. Crucially, neither read path re-verifies values: `lookup_by_index` returns every record whose 25-byte key prefix matches with no value comparison, and `check_unique_key` treats any existing entry under the hash key as a duplicate — the hash **is** the constraint.
- **Failure scenario:** a tenant with INSERT/SELECT privilege (1) crafts values whose dual-FxHash collides with a victim value → `lookup_by_index` returns the attacker's records for queries on the victim's value (wrong results / cross-row data confusion); (2) crafts a colliding value on a UNIQUE column → legitimate inserts of the victim value are rejected with `DuplicateKey` (availability); (3) floods distinct colliding values into one 25-byte key → posting cache and prefix scan degrade to O(n) per lookup (hash-flooding DoS — exactly what `RandomState` exists to prevent).
- **Fix:** For the unique family at minimum, re-verify on hit: store the (serialized) indexed values (or a strong digest) alongside the `RecordId` in the posting value and compare before returning `DuplicateKey` (mirroring how `extract_index_leaves` already exists for value comparison). Longer term, switch the value-hash to a keyed or cryptographic digest (e.g. SipHash-1-3 with a per-table key, or blake3 truncated to 16 bytes) — the tag-stable encoding scheme can stay. Add an adversarial-collision test; the current suite only checks that two *different* strings hash differently.

### 3.2 — medium — FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`)
- **File:line:** `crates/shamir-index/src/tokenizer.rs:462-469`, used at `fts_backend.rs:70-93,125-131,204-211` and `fts_ranked_backend.rs:80-99,150-156`.
- **Issue:** Every token from user documents (`tokenize_record`) and query strings (`tokenize_query`) is compressed to a `u64` via raw `FxHasher` and that `u64` **is** the token identity inside the posting key (`[index_id][FTS][hash8][record_id16]`). FxHash is trivially collidable on short attacker-chosen strings. The scan-side filter (`pk.index_id == … && pk.type_tag == FTS`) verifies the tag but never the token itself — a collision merges two tokens' posting lists with no runtime detection.
- **Failure scenario:** an attacker who can insert documents computes a token colliding with a victim term (a rival product name, another user's handle) and seeds documents with it; every subsequent FTS query for the victim term returns the attacker's records (search-result poisoning), and BM25 ranking (`fts_ranked_backend.rs:313`) scores the injected postings as if they were the queried term. A flood of colliding tokens also funnels all postings into one prefix scan (per-query O(total postings of the bucket)).
- **Fix:** Widen the token identity to a 128-bit digest (the posting layout has room: `FIXED_OVERHEAD` already treats `value_bytes` as variable — store 16 bytes instead of 8), using a keyed or cryptographic hash (SipHash-1-3 keyed per-DB, or blake3-128). Keep `token_hash` only for non-adversarial internal uses. Either re-affirm CLAUDE.md pillar 4 with an explicit threat-model note on `token_hash` (documents are attacker-controlled text) or upgrade the hash.

### 3.3 — medium — `trusted_pure` scalar gate is documented here but not enforced at this crate's dispatch boundary
- **File:line:** `crates/shamir-index/src/expr.rs:47-52,167-185` (`IndexExpr::Scalar` → `resolver.call(name, …)`), `crates/shamir-index/src/functional_backend.rs:52-66,95-105`, `crates/shamir-index/src/build_backend.rs:22-51`.
- **Issue:** `expr.rs` states "Only `.trusted_pure()`-vouched scalars are allowed here", and `shamir-funclib::registry` documents `is_indexable()` as "the index-safety gate". That gate is enforced exactly once, in `shamir-engine` (`table_manager_index_mgmt.rs:259`), at CREATE INDEX time. The eval path in *this* crate calls `resolver.call(name, ...)` against the **full** resolver — user layer first, then every builtin including non-vouched, non-deterministic ones (`uuid_v4`, `now`) — with no `is_indexable()` check. The scalar name travels in the bincode-persisted `IndexDescriptor`, and `build_index2_backend_with_resolver` re-arms evaluation from disk on every open.
- **Failure scenario:** (a) a tampered/legacy `__meta__/indexes` blob carrying `Scalar { name: "uuid_v4" }` (or any host-registered function) is loaded on open and evaluated on every write and lookup of the functional index — write-path and read-path hashes diverge, so the index silently returns wrong/empty results while appearing `Ready`, and an un-vouched host closure is dispatched from persisted data. (b) A scalar re-registered after CREATE without `.trusted_pure()` (same name, new behavior) is picked up on reopen with no gate.
- **Fix:** In `IndexExpr::eval_with_scalars` (or `FunctionalBackend::eval_or_null`), resolve via `resolver.get(name)` and reject with `ExprError::ScalarError` unless `entry.is_indexable()`, then dispatch the entry directly — making the boundary self-defending regardless of which layer validated the DDL, at the cost of one map lookup per scalar-node eval.

### 3.4 — medium — External vector-backend API key persisted in cleartext inside the index-metadata blob
- **File:line:** `crates/shamir-index/src/kind.rs:189-200` (`VectorBackendRef::External { driver, url, api_key_secret }`), persisted via `crates/shamir-index/src/persistence.rs:93-106` → `meta_envelope.rs:50-52` (plain bincode in `MetaEnvelope`, no confidentiality or MAC).
- **Issue:** `SecretString` redacts `Debug` and zeroizes on drop (`shamir-types/src/secret.rs`), but its `Serialize` is pass-through, so `save_index2_metadata` writes the raw API key into the `system:_m.idx` record of the info store. The envelope provides versioning only — no encryption, no MAC. Anyone with read access to the info store (file-level access, a backup, a replicated/mis-scoped store) recovers the credential; the in-memory protections never engage for the at-rest copy.
- **Failure scenario:** operator configures a vector index against an external service with an API key; a store snapshot/backup or an info-store read primitive leaks the third-party credential in plaintext.
- **Fix:** Either exclude `api_key_secret` from the persisted descriptor (like `VectorConfig::quantization` already does with `#[serde(skip)]` — resolve the secret at runtime from a keyring/env reference), or persist a *reference* (secret name) rather than the value. If at-rest encryption exists at the storage layer, document that contract at this field.

### 3.5 — low — Snapshot load joins persisted `basename`/`qbasename` into temp file paths unsanitized; manifest/sidecar carry no integrity check
- **File:line:** `crates/shamir-index/src/vector/snapshot.rs:871-917` (f32 path: `load_dir.path().join(format!("{basename}.hnsw.graph"))` at 907-908) and `:971-1013` (u8 path: `qbasename` at 983-984); acknowledged no-CRC on manifest/sidecar at `snapshot.rs:694-697`.
- **Issue:** Every *chunk* is CRC32-verified, but the manifest and sidecar themselves are only magic/version-checked (the in-code comment admits "they carry no crc of their own"). `basename` is a free-form `String` read from that unverified manifest and interpolated into a filesystem path. On Windows/Unix alike, a basename containing `..`, `../`, or absolute-path components escapes the `TempDir` in `File::create` + `write_all` during load — an arbitrary-location overwrite (content: attacker-influenced graph bytes) at table open.
- **Failure scenario:** an attacker with write access to the info store (the same trust level the per-chunk CRCs already defend against) edits only the manifest (no chunk CRC to recompute, no envelope MAC to forge) to set `basename = "..\..\Users\Public\x"`; on next open, the load path writes `x.hnsw.graph` outside the temp dir.
- **Fix:** Sanitize/validate `basename` on load (reject path separators, `..`, NUL, or any non-identifier character — the dump side only ever produces `"shamir"`/`"shamirq"` + uniquifier suffixes), or ignore the persisted name and derive it locally. Optionally fold a CRC over the manifest/sidecar payloads (see also 5.7).

### 3.6 — low — `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token
- **File:line:** `crates/shamir-index/src/tokenizer.rs:110-170` (`NgramTokenizer` / `emit_ngrams`), consumed unbounded at `fts_ranked_backend.rs:80-99` and `plan_insert:163-185`.
- **Issue:** A single alphanumeric run of length L emits ~L owned `String` n-grams (each a fresh allocation), and every gram becomes a separate `SetPosting` op in the planned batch. There is no cap on field text length, tokens per document, or ops per `plan_insert` anywhere in the FTS path; `doc_len: u32` also saturates BM25 stats for absurd inputs.
- **Failure scenario:** a writer inserts a record whose indexed field is a multi-MB unbroken string (cheap for the attacker, one field) into an n-gram-indexed table: tokenization allocates thousands of small strings per KB of input, and the commit plans millions of posting ops — memory spike + store write amplification on the server.
- **Fix:** Cap per-field token count / total gram count (tunable, e.g. 100k tokens) with truncation or a typed `IndexError`, and/or cap grams emitted per word; the FTS write path is the right chokepoint since the value arrives as an opaque `&str`.

### 3.7 — nit — NEON kernels read `u32` through `*const u8`-derived pointers — aligned-load safety contract violated (aarch64 only, untested on CI)
- **File:line:** `crates/shamir-index/src/vector/simd.rs:932-933` (`weighted_bilinear_neon`), `:1101-1102` (`weighted_sq_diff_neon`), `:1250` (`weighted_linear_neon`).
- **Issue:** `vld1_lane_u32(xp.add(i) as *const u32, …)` loads a `u32` from a pointer obtained via `&[u8]::as_ptr()`, whose alignment guarantee is 1. The `std::arch` intrinsic requires the pointer to be valid for an *aligned* `u32` read; with a `Vec<u8>` code buffer at a non-4-aligned address this is formally UB. The module's own header concedes these kernels are never executed on the CI hosts, so the misalignment case is also untested. (The x86 paths correctly use `_mm_loadu_*` unaligned intrinsics; loop bounds are in-range.)
- **Failure scenario:** on aarch64 with a misaligned `vectors_u8` sub-slice, execution hits an alignment violation — in practice hardware tolerates it, but the contract breach is exactly the class of latent UB that surfaces under future compiler optimization.
- **Fix:** Load with `vld1_u8`/`vld1q_u8` (unaligned-safe, as `dot_u8_neon_wide` right below already does) and widen with `vmovl_u8`, or `read_unaligned` into a `u32` before `vdup_n_u32`. Add an aarch64 CI leg or an alignment-fuzz unit test via a scalar twin.

**Clean for the record:** all `unsafe` confined to `vector/simd.rs` with runtime feature detection; posting/sorted key decoders bounds-checked and `Option`-returning; `posting_layout`, `ddl_op_log` (versioned), `decode_covering_projection`, `IndexInfo::decode_bytes`, snapshot chunk reassembly all fail closed on malformed input; vector search inputs consistently clamped (`MAX_TOPK`, `MAX_EF_SEARCH`, dim checks, atomic batch dim validation); no secret-dependent comparisons (distances/keys are not secrets); `RecordId::system` key-collision hazards explicitly documented and byte-verified at every new system key; CRC32 as snapshot integrity matches the "checksums everywhere" corruption-detection pillar.

---

## 4. performance-hotpath

### 4.1 — high — Per-row deep-clone of the whole index-definition set in every write planner
- **File:line:** `crates/shamir-index/src/base_index/index_info.rs:310-315` (`IndexInfo::iter`), `crates/shamir-index/src/base_index/sorted_index_manager.rs:544-550` (`iter_indexes`); call sites `index_manager.rs:2457/2581/2683`, `index_manager_unique.rs:95/175/218/281/473/504/547/1007/1037/1092`, `sorted_index_manager.rs:1674/1754/1827`.
- **Issue:** `IndexInfo::iter()` clones every `IndexDefinition` on every call (`snap[i].clone()` — one `Vec<IndexInfoItem>` + one `Vec<u64>` per path → heap allocations per def), and `SortedIndexManager::iter_indexes()` deep-clones the entire `Vec<SortedIndexDefinition>` (~4 heap `Vec`s per def). These are called from `plan_record_created/updated/deleted` (hash), `plan_record_*_unique` / `validate_unique_for_*` (unique — the validate paths additionally `collect()` into a fresh `Vec` per call), and `plan_record_*` (sorted). Every insert/update/delete on an indexed table pays ~I×(P+1) allocations (I = indexes, P = paths/index) purely to read definitions that change only at DDL time — a direct pillar-3 violation ("allocation in loops") on the hottest write path in the crate. The RCU snapshot `Arc` is already held by the iterator; only the element clones are wasted.
- **Failure scenario:** bulk import of 1M rows into a table with 5 indexes (2 paths each) → ~15M+ avoidable heap allocations on top of the postings' own allocator pressure.
- **Fix:** Add a borrowing iterator (or return `Arc<Vec<Def>>` / `Arc<[Def]>` from `load_local()`), have the planners iterate by reference; for the unique validators, thread a batch-scope def snapshot the way `validate_unique_for_create_with_defs` already models.

### 4.2 — high — Sorted/unique apply paths issue one store round-trip per posting — the transact batching landed only for the regular-hash family
- **File:line:** `crates/shamir-index/src/base_index/sorted_index_manager.rs:1847-1864` (`apply_ops`), `crates/shamir-index/src/base_index/index_manager_unique.rs:473-481/504-529/547-554`; contrast `crates/shamir-index/src/base_index/index_manager.rs:2729-2746`.
- **Issue:** `IndexManager::apply_ops` (regular-hash) documents and implements collapsing all `SetPosting`/`RemovePosting` ops into ONE ordered `Store::transact` ("one fsync instead of N per-key writes"). The sibling families never got the same fix: `SortedIndexManager::apply_ops` loops `store.set(...).await` / `store.remove(...).await` per op, and the unique `on_record_*_unique` handlers await `add_unique_entry_by_key`/`remove_unique_entry_by_key` per def. On the non-tx direct CRUD path, a row update touching a sorted index produces 2 sequential store round-trips (remove+set), and a table with N indexes pays N sequential round-trips (each potentially its own fsync on durable backends) where the hash family pays 1.
- **Failure scenario:** update-heavy workload on a table with a sorted index and 2 unique indexes on a durable backend: 3-5 sequential fsyncs per row write vs 1 for an equivalent hash-only table; write throughput scales O(index_count) in round-trips.
- **Fix:** Mirror `IndexManager::apply_ops`: fold the ops into one `Store::transact` (or `set_many`/`remove_many`) batch, preserving op order for last-write-wins.

### 4.3 — medium — FTS lookup materializes every matching posting list and sorts all results — no top-k bound
- **File:line:** `crates/shamir-index/src/fts_ranked_backend.rs:297-380` (`lookup`), `crates/shamir-index/src/fts_backend.rs:202-232`.
- **Issue:** `FtsRankedBackend::lookup` (a) buffers the FULL posting list of every query token into `Vec<(RecordId, FtsPostingValue)>` (`scan_token_with_values`), (b) for `AndAll` builds a second `Vec<BTreeSet<RecordId>>` (`rid_sets`) and folds pairwise intersections — each fold step allocating a fresh `BTreeSet` — (c) probes `intersection.contains(rid)` (O(log M)) per posting, then (d) collects ALL surviving scores into a `TFxMap` and fully sorts them (`ranked.sort_by`) even though BM25 queries are top-k by construction. `IndexResult::Ranked(Vec)` is unbounded: every matching document is returned. `FtsBackend::lookup` has the same materialize-then-fold shape.
- **Failure scenario:** ranked search for a frequent term (or an `OrAny` of two mid-frequency terms) on a 10M-doc table buffers and sorts tens of millions of entries per query — latency and peak memory are O(matching postings), not O(k); repeated concurrent queries multiply the transient.
- **Fix:** Score inside the scan stream and keep a bounded max-heap of size k (the exact pattern `brute_force.rs::push_topk` already implements); for `AndAll`, stream-merge the per-token scans (postings are record-id-ordered within a token) instead of materializing full sets; drop the intermediate `rid_sets` Vec.

### 4.4 — medium — `FtsRankedBackend::plan_update` tokenizes the old record twice
- **File:line:** `crates/shamir-index/src/fts_ranked_backend.rs:193-195`.
- **Issue:** `let old_set = self.tokenize_set(old)` internally calls `tokenize_with_freq(old)` and throws the frequencies away; two lines later `let (_, old_doc_len) = self.tokenize_with_freq(old)` re-runs the full pipeline on the same record. With the `Full` tokenizer (lowercase + stopword filter + Snowball stemming, one heap allocation per token / per n-gram) this doubles the single most expensive CPU step of the FTS update hot path.
- **Failure scenario:** every UPDATE of an FTS-indexed text column pays 2× tokenization of the old document — a constant-factor write-throughput loss profiling will attribute straight to `plan_update`.
- **Fix:** Call `tokenize_with_freq(old)` once; derive `old_set` from `freq.keys()` (exactly what `tokenize_set` does internally).

### 4.5 — medium — `IndexExpr::Scalar` evaluation constructs a fresh `Interner` per row
- **File:line:** `crates/shamir-index/src/expr.rs:172` (inside `eval_with_scalars`, Scalar arm).
- **Issue:** every evaluation of a scalar-backed functional index builds `Interner::new()` as a scratch interner for the `InnerValue → QueryValue → InnerValue` conversion. This runs on every insert/update/delete planned against such an index (`functional_backend.rs::eval_or_null` → `plan_insert`/`plan_update`/`plan_delete`) — a per-row, per-op construction of the interner's internal map structures, even though the doc comment notes scalar leaves don't need it at all.
- **Failure scenario:** a table indexed on a `.trusted_pure()` scalar expression under sustained write load pays interner construction + two owned value conversions per row on the write hot path — pure allocation overhead invisible at the API level.
- **Fix:** Hoist to a `thread_local!` scratch interner (cleared per use), or construct lazily only on the Map/List arms that actually need interning.

### 4.6 — medium — Vector snapshot dump/load fully materializes the graph and sidecar in RAM
- **File:line:** `crates/shamir-index/src/vector/snapshot.rs:404-435` (`fs::read` both dump files whole), `440-445 + 649-688` (all chunk ops accumulated into one `Vec<KvOp>` held until `store.transact` at 642), `529-549` (sidecar clones every live vector), `788-792` (load reassembles whole files), `884-888` (sidecar fields `.clone()`d again before map insertion).
- **Issue:** `dump_snapshot_with_gen` reads both `hnsw_rs` dump files entirely into memory, then re-copies each 1 MiB chunk (`chunk.to_vec()` + bincode re-encode) into a single `ops: Vec<KvOp>` that lives until one final `transact` — peak transient ≈ 2-3× the dump size, plus a full duplicate of every live f32 vector for the sidecar (`for_each_vector(... v.to_vec())`). This runs inside the single-flight background snapshot task after every `VECTOR_SNAPSHOT_DELTA_THRESHOLD` mutations, so the spike recurs periodically in steady state. The load path reassembles the whole files from `get_many` results AND clones the entire sidecar (fields never used again) — boot-time peak ≈ 2-3× index size.
- **Failure scenario:** a 5M-vector dim-128 index (GB-scale dump) triggers a background snapshot that buffers the whole dump + chunk ops + sidecar vectors concurrently — an O(index-size) RSS spike on a tokio task, unrelated to query or write load, that can OOM a memory-sized server.
- **Fix:** Stream the dump files in 1 MiB slices and write chunk batches via repeated `transact`/`set_many`, keeping only the manifest write atomic (the flip already provides atomicity); move the sidecar fields out of the owned `sidecar` (`std::mem::take`) instead of cloning; write sidecar vectors in bounded batches.

### 4.7 — medium — Vector delta-log replay on restart applies ops one at a time
- **File:line:** `crates/shamir-index/src/vector/snapshot.rs:1250-1266` (`replay_delta`).
- **Issue:** each `DeltaOp::Upsert` is replayed via a single `adapter.upsert(rid, &vec).await` — on the HNSW f32 path that is one `spawn_blocking` hop + one single-node `hnsw.insert` per op, all strictly sequential. After a crash with up to `VECTOR_SNAPSHOT_DELTA_THRESHOLD` (default 10k) unabsorbed mutations, restart replay does thousands of isolated single-node inserts where `upsert_batch` (one rayon `parallel_insert` — the primitive `rebuild()` already uses per 1k-row page) would parallelize them.
- **Failure scenario:** node restarts after heavy write churn; table-open latency grows linearly with pending delta ops × per-op blocking-pool hop instead of amortizing via batch inserts — the snapshot's whole point (O(load) fast open) degrades toward the full-rebuild path it was meant to avoid.
- **Fix:** Accumulate ops per chunk (or across all chunks) into a `Vec<(RecordId, Vec<f32>)>` and replay via `adapter.upsert_batch`, applying deletes in their original order relative to upserts (e.g. batch between delete boundaries).

### 4.8 — medium — Posting cache is bounded by entry count, not bytes
- **File:line:** `crates/shamir-index/src/base_index/index_manager.rs:60` (`POSTING_CACHE_CAP = 512`), `2868-2881` (count-only eviction).
- **Issue:** the posting cache caps at 512 entries of `Arc<[RecordId]>` with no per-entry or total byte budget. A low-cardinality index (boolean flag, `status` enum) has O(rows/distinct) postings per entry; caching 512 such entries pins `512 × avg_postings × 16` bytes with no upper bound tied to rows. The doc's mitigation ("typical workloads concentrate on a handful of values") is an assumption, not an enforcement — and the workload where the cache helps MOST (low cardinality, repeated equality lookups) is exactly the one where each entry is huge.
- **Failure scenario:** 20M-row table, indexed `status` column with 8 values → each cached entry ~40MB (2.5M ids × 16B); a secondary low-cardinality index pushes the cache to hundreds of MB–GBs of pinned RSS that never evicts because 8 ≪ 512 entries.
- **Fix:** Add a byte budget (Σ `entry.len() × 16` tracked in an `AtomicUsize`, decremented on eviction/invalidation) alongside the entry cap; evict largest-first or arbitrarily once the budget is crossed.

### 4.9 — low — `HnswAdapter` f32 small-index search clones every vector per query
- **File:line:** `crates/shamir-index/src/vector/hnsw_adapter.rs:2831-2835`.
- **Issue:** the `len() <= BRUTE_FORCE_MAX` (256) exact-search branch clones every stored vector (`pairs.push((*i, v.clone()))` — one heap `Vec<f32>` per element per query) before scoring. The quantized twin `search_quantized_bruteforce` (1805-1827) was already fixed by audit #530 to score inside the `iter_sync` closure with zero per-candidate allocation; the f32 branch kept the clone-per-candidate shape.
- **Failure scenario:** small warm index (200 vectors, dim 768) under sustained query load pays 200 heap clones of 3KB each per query — allocation churn exactly where the design intends a cheap deterministic microsecond scan.
- **Fix:** Mirror the #530 fix: collect `(usize, f32)` scored pairs inside the scan callback.

### 4.10 — low — SQ8 fit runs inline on the threshold-crossing write — O(N) stall on one upsert
- **File:line:** `crates/shamir-index/src/vector/hnsw_adapter.rs:2470-2475` (trigger sites), `1292-1685` (`try_fit_and_rebuild`), catch-up loop `1567-1620`.
- **Issue:** the upsert that crosses `FIT_THRESHOLD` awaits the entire fit inline: O(N) quantizer training, a full `parallel_insert` graph build, and a catch-up loop that repeatedly rescans `vectors` (two full `iter_sync` passes per spin, re-quantizing still-pending entries each iteration) with a spin/backoff wait. One-shot and single-flight by design, but the crossing writer absorbs the whole transition as tail latency; the surrounding machinery (snapshot/compaction triggers) already demonstrates the background-task pattern.
- **Failure scenario:** p99.9 write latency spikes by the full fit duration (tens of ms+ at N≈256×dim) exactly once per adapter, per compaction cycle — a periodic outlier in write-latency histograms.
- **Fix:** Keep the crossing upsert on the f32 path (correct pre-fit) and hand the fit to a single-flight `tokio::spawn` (mirroring `trigger_snapshot_check`), letting post-fit upserts flip when `is_fitted` lands. (The error-swallowing at the same trigger sites is 6.5.)

### 4.11 — low — `BruteForceAdapter` deep-clones the whole snapshot on every drained write batch
- **File:line:** `crates/shamir-index/src/vector/brute_force.rs:84-93` (publish per drain), `175-182` (`clone_snap`).
- **Issue:** every publish does a full deep clone of `rids` + `vecs` (N heap `Vec<f32>` clones) + `norms` + `index` — O(N·dim) copy per drained batch. Coalescing amortizes bursts, but a steady one-write-at-a-time stream publishes per write. The adapter is used by tests, benches, and `shamir-engine`'s `vector_report.rs` example / `vector_search.rs` bench ground-truth — not by production `build_index2_backend` — hence low severity, but the bench's 10k-element ground-truth inserts are silently O(N²·dim) in setup cost.
- **Failure scenario:** the vector-search bench ground-truth setup takes O(N²·dim) time — silently inflating bench wall-clock and masking regressions elsewhere.
- **Fix:** Store `Arc<Vec<f32>>` per vector (clone the outer Vec of Arcs + index only) or use a persistent/immutable structure so publish is O(1) per unchanged element.

### 4.12 — nit — `apply_index_ops_at_commit` does a linear backend find per provenance group
- **File:line:** `crates/shamir-index/src/write_ops.rs:184-196`.
- **Issue:** for each distinct `(name_interned, epoch)` group the commit path scans `backends.iter().find(...)` — O(groups × backends) per commit, with typical single-digit counts. Harmless today; would matter only if per-table backend counts grew.
- **Failure scenario:** none at current scale.
- **Fix:** A small `TFxMap<u64, &backend>` built once per call removes it.

**Coverage note (from the lens, still true):** the crate's benches honestly measure the already-fixed paths (`posting_cache_hit`, `create_index_streaming`, `sq8_hot_path`, `reader_drain_gate`). None of findings 4.1-4.8 have a bench or test exercising their cost; the FTS lookup path, the per-row def-clone, the sorted/unique apply fan-out, and the snapshot dump memory spike are all currently unmeasured.

---

## 5. api-wire-protocol

No `serde_json` dependency anywhere; all lookups go through the typed `IndexQuery` enum and all write ops through typed `IndexWriteOp`s — the builder-only rule is trivially satisfied. The base_index family's on-disk versioning is exemplary. The serious problems concentrate in the index2 vector family's persisted config/codec contracts.

### 5.1 — high — Persisted `VectorConfig.backend` is ignored on the reopen/rebuild path
- **File:line:** `crates/shamir-index/src/build_backend.rs:52-65` (with `crates/shamir-index/src/kind.rs:189-200`) — verified during synthesis (hardcoded `HnswConfig { max_elements: 100_000, m: 16, ef_construction: 200, ef_search: 50 }`).
- **Issue:** `build_index2_backend_with_resolver` — documented as "Shared by `TableManager::create` (reopen path) and `replicate_index2_descriptors_from`" — never reads `cfg.backend`. Grep confirms no production code in the crate ever reads `VectorBackendRef`: `InProcessHnsw { ef_construct, m }` is constructed only in tests, and `External { driver, url, api_key_secret }` is silently rebuilt as an in-process HNSW. The persisted wire discriminant says one thing; the restored backend is another.
- **Failure scenario:** (a) `CREATE … VECTOR (ef_construct = 400, m = 32)`; crash before the first background snapshot (snapshots only trigger at `VECTOR_SNAPSHOT_DELTA_THRESHOLD` mutations); reopen takes the `NotFound`/rebuild branch → graph rebuilt with m=16/ef=200 → recall silently degrades, and the *next* snapshot persists the default params, making the loss permanent. (b) An `External` driver index reopens as in-process HNSW with no error. Existing tests mask this only by coincidence: every test descriptor uses `ef_construct: 200` (`vector_restore_tests.rs:110-111`, `crash_recovery_tests.rs:121-122`, `delta_log_tests.rs:93-94`), exactly matching the hardcoded value.
- **Fix:** In the `IndexKind::Vector(cfg)` arm, map `cfg.backend` → `HnswConfig` (`InProcessHnsw { ef_construct, m }` → config; `External { .. }` → explicit error or a real external adapter), and thread `cfg.quantization` through `HnswAdapter::new_with_quantization` (see 5.2). Add a reopen test with non-default `ef_construct`/`m`.
- **Dedup:** same root defect flagged by correctness-tdd #8 (medium) and performance-hotpath #11 (low) — counted once here as the highest-severity/most complete statement.

### 5.2 — high — SQ8 quantization opt-in has no durable carrier and is lost on most restarts
- **File:line:** `crates/shamir-index/src/kind.rs:168-186`; `crates/shamir-index/src/build_backend.rs:53-63`; `crates/shamir-index/src/vector/hnsw_adapter.rs:526-549` and `:1292-1296`.
- **Issue:** `VectorConfig::quantization` is `#[serde(skip)]`, so it is absent from every persisted `IndexDescriptor`; the kind.rs doc says the mode is "carried through the WIRE op … NOT persisted in #411 (snapshot codec for quantization is #412)". But #412 only round-trips a *fitted* quantizer through a v2 snapshot sidecar. The two restore paths that don't go through a quantized snapshot both construct unquantized adapters: `build_index2_backend` calls `HnswAdapter::new` (never `new_with_quantization`), and `HnswAdapter::from_parts` hardcodes `quantization: None` (`:549`) — while `try_fit_and_rebuild` returns immediately when `quantization.is_none()` (`:1293-1296`). The `from_parts` doc's claim that a loaded adapter "starts un-fitted and will re-fit at the threshold on the next upserts" (`:526-530`) is therefore false: an adapter loaded via `from_parts` can never fit.
- **Failure scenario:** create an SQ8 index; insert ≥256 vectors (fit fires, u8 graph live). Restart *before* a background snapshot exists (pre-threshold), or with a corrupt/version-mismatched snapshot → rebuild path → unquantized f32 adapter forever; the next dump is non-quantized, so every later restart stays f32. Result: silent ~4× memory regression and changed recall characteristics on a feature the user explicitly opted into. Even a *successful* restart from a snapshot taken pre-fit permanently loses the opt-in.
- **Fix:** Persist the quantization mode durably — replace `#[serde(skip)]` with a forward-compat carrier (e.g. encode it into the existing `IndexDescriptor.options` bytes (5.9), or bump `PersistedIndexes` with a shadow-shape fallback like `state` got in F-50), or reconstruct it in `from_parts`/`build_index2_backend` from a config source that survives restart. Also fix the `from_parts` doc comment.

### 5.3 — medium — Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous
- **File:line:** `crates/shamir-index/src/vector/snapshot.rs:95-105`, `:278-310`, `:206-255`; test at `crates/shamir-index/src/vector/tests/quantization_snapshot_tests.rs:363-406`.
- **Issue:** `SNAPSHOT_SUPPORTED_VERSIONS = &[1, 2]` and the docs promise "A v1 dump loads on a v2 build via the back-compat path". But the v2 fields were *inserted into the middle* of the positional bincode layout — `SnapshotManifest` gained `qgraph_chunks`/`qdata_chunks` between `data_chunks` and `basename`, and `qbasename` between `basename` and `delta_applied_upto`; `SnapshotSidecar` gained `quantization`/`vectors_u8` before `sections_crc32` — guarded only by `#[serde(default)]`, which this workspace has *proven* (state.rs module doc, `index_state_compat_tests.rs`) does not rescue old bincode blobs (and mid-insertion misaligns regardless). Unlike `persistence::decode_persisted_indexes`, `IndexInfo::decode_bytes`, and `SortedIndexManager::load`, there is no v1 shadow-shape fallback. A genuine pre-#412 blob fails inside `MetaEnvelope::open` → mapped by `map_meta_err` to `Corrupt` — never reaching the `SNAPSHOT_SUPPORTED_VERSIONS` check, so `1` in that array is dead. The regression test `migration_v1_snapshot_loads_back_compat` does not write v1-shaped bytes: it decodes a current-shape dump, sets `format_version = 1`, and re-encodes with the *current* struct — it validates only the version-label gate, not the layout.
- **Failure scenario:** upgrade a deployment whose vector indexes have pre-#412 snapshots: every open logs "snapshot load failed … falling back to full rebuild" and pays a full O(N) re-index per vector index, while the code and `SNAPSHOT_SUPPORTED_VERSIONS` claim v1 loads are supported.
- **Fix:** Either add a v1 shadow-shape decode (mirror `decode_persisted_indexes`) or stop advertising v1 support (drop it from the array + docs) and let `VersionMismatch`-style messaging surface. Add a test fixture that writes actual v1-shaped bytes.
- **Caveat (from the lens, unresolved):** if the v1 writer already serialized the now-`#[serde(default)]` fields as zero/None, the layout may in fact be compatible — but nothing in the code or tests demonstrates that, and the `#[serde(default)]` annotations argue otherwise. Either way the contract is currently unverified.

### 5.4 — medium — Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dependency
- **File:line:** `crates/shamir-index/src/tokenizer.rs:462-469` (`token_hash`); `crates/shamir-index/src/base_index/index_keys.rs:186-240` (`hash1`/`hash2`); `crates/shamir-index/src/functional_backend.rs:68-81`, `:131-175` (`hash_value`/`hash_inner`); `Cargo.toml:30` (`rustc-hash = "2.1"`) vs `Cargo.toml:47-51` (hnsw_rs exact pin).
- **Issue:** FxHasher-derived u64s are embedded in persisted keys for three families: FTS postings (`token_hash`), legacy regular/unique postings (`hash1`/`hash2`, whose tag scheme is explicitly documented as "part of on-disk index compatibility"), and functional postings (`FunctionalBackend::hash_value`, doc: "the tag scheme is part of on-disk index compatibility and must stay stable"). The *tags* are stable, but the hash function's output is not under this repo's control: `rustc-hash` gives no cross-version output-stability guarantee (and did change output at the 1.x→2.x boundary), yet it is caret-pinned and unmarked in any format version. The team already solved this exact risk class for `hnsw_rs` (exact `=0.3.4` pin + `HNSW_RS_VERSION` check refusing foreign dumps at load) — `rustc-hash` got neither.
- **Failure scenario:** a routine `cargo update` bumps rustc-hash 2.1 → 2.2 with an algorithm tweak. Queries now hash tokens/values differently than the on-disk keys: all FTS/functional/hash-index lookups silently return empty/partial results. `legacy_indexes_need_rebuild` does not fire (the stored `_m.idx.lfv` still equals 2 — the *scheme* tag didn't change), so nothing rebuilds.
- **Fix:** Exact-pin `rustc-hash` (mirroring the hnsw_rs pin comment) and/or fold the hash-function identity into `LEGACY_INDEX_FORMAT_VERSION` with a boot-time self-check (hash a fixed vector at startup, compare against a baked-in constant; bump the format version on mismatch to trigger rebuild). Related but distinct: the adversarial-collision findings 3.1/3.2.

### 5.5 — medium — `flip_generation` never prunes the old generation's `qgraph`/`qdata` chunks
- **File:line:** `crates/shamir-index/src/vector/snapshot.rs:1287-1330`; caller `crates/shamir-index/src/vector/vector_backend.rs:906-911`, `:955-963`.
- **Issue:** The generation flip is the *only* prune mechanism (the dump path's comment "Wipe any PRIOR generation's chunks first" is followed by no code). `flip_generation` removes the old gen's `graph` chunks, `data` chunks, and sidecar — but not its `qgraph`/`qdata` chunks. The caller reads only `(m.gen, m.graph_chunks, m.data_chunks)` from the old manifest, so the API couldn't even express the q-chunk counts: the signature has `old_graph_chunks`/`old_data_chunks` but no `old_qgraph_chunks`/`old_qdata_chunks`.
- **Failure scenario:** a quantized index flips generation every `VECTOR_SNAPSHOT_DELTA_THRESHOLD` mutations; each flip leaks the previous u8-graph dump (a full second copy of the graph) in the info store forever. Unbounded storage growth with no correctness signal (the manifest points only at the new gen, so nothing ever reads the orphans).
- **Fix:** Extend the manifest read + `flip_generation` signature with the old q-chunk counts and emit `KvOp::Remove` for `qgraph`/`qdata` chunk keys in the same transact.

### 5.6 — low — `MetaEnvelope::open` validates magic/version only after deserializing the payload
- **File:line:** `crates/shamir-index/src/meta_envelope.rs:50-67`.
- **Issue:** The module doc says the envelope exists "so future migrations can dispatch on `version` without ambiguity", but `open` runs `bincode::deserialize::<MetaEnvelope<T>>` over the *whole* envelope first. A future version-2 envelope whose payload `T` changed shape fails inside the payload decode and surfaces as `MetaError::Decode` (mapped to `Corrupt` by `map_meta_err`) — `UnsupportedVersion` is observable only when the payload happens to still decode. This is precisely why every consumer (`decode_persisted_indexes`, `IndexInfo::decode_bytes`, sorted three-tier load) had to grow shadow-shape fallback chains instead of dispatching on the version field.
- **Failure scenario:** the next wire-format bump re-creates the shadow-shape-fallback sprawl the envelope was designed to prevent.
- **Fix:** Split the envelope into a fixed header (`magic`, `version`, `written_at_nanos`) decoded first, with the payload decoded only after the version check.

### 5.7 — low — Snapshot load path can panic on corrupt-but-decodable persisted data; the sidecar carries no checksum
- **File:line:** `crates/shamir-index/src/vector/quant_meta.rs:58-80`; `crates/shamir-index/src/vector/sq8.rs:89-93`; `crates/shamir-index/src/vector/snapshot.rs:694-706`.
- **Issue:** `QuantMeta::to_quantizer` `assert_eq!`s the method tag and the `mins`/`scales` lengths, and calls `Sq8Quantizer::fit`, which `assert!`s `dim > 0`. The method tag is guarded upstream (snapshot.rs:822), but a sidecar blob whose `dim`/`mins`/`scales` disagree (still-valid bincode — e.g. after a bit flip) panics during `restore_on_open`, i.e. a process crash driven purely by disk bytes. The input is effectively unverified: per `map_meta_err`'s own doc, "the manifest/sidecar bytes … carry no crc of their own, unlike the chunk bodies". CLAUDE.md reserves panics for programmer-error invariants, not external data.
- **Failure scenario:** a bit-flipped sidecar survives envelope magic/version checks and bincode decode, reaches the asserts, and crash-loops the table at open — where every other snapshot-corruption class surfaces as `SnapshotError::Corrupt` + full rebuild.
- **Fix:** Make `to_quantizer` fallible (or validate lengths/dim in `load_snapshot` and return `SnapshotError::Corrupt`), preserving the warn + full-rebuild fallback; consider a crc32 over the sidecar/manifest envelope payloads (pairs with 3.5).
- **Dedup:** same root defect flagged by error-handling-lifecycle #11 — counted once here.

### 5.8 — low — Bincode ordinal-stability contract is documented on some persisted enums but missing on others
- **File:line:** `crates/shamir-index/src/kind.rs:10-40` (`IndexKind`, `TokenizerKind`); `crates/shamir-index/src/expr.rs:21-52` (`IndexExpr`).
- **Issue:** `StemLanguage`, `VectorQuantization`, and `IndexState` each carry an explicit "# Bincode ordinal stability: append only / DO NOT MOVE" contract. `IndexKind`, `TokenizerKind`, and the entire persisted `IndexExpr` AST (inside `FunctionalConfig`, inside `IndexDescriptor.kind`) are serialized by the same ordinal-tagged bincode but carry no such note. `IndexExpr` in particular reads like an alphabetized list where inserting a variant mid-enum is an easy "innocent" refactor that silently corrupts every persisted functional index.
- **Failure scenario:** an "innocent" mid-enum insertion corrupts every persisted descriptor/functional index on next open.
- **Fix:** Replicate the ordinal-stability doc + `// ordinal N — DO NOT MOVE` markers on `IndexKind`, `TokenizerKind`, and `IndexExpr` variants.

### 5.9 — low — `IndexDescriptor.options` is dead public API
- **File:line:** `crates/shamir-index/src/descriptor.rs:26-29`.
- **Issue:** Documented as "Opaque backend-specific tuning (bincode-friendly)… empty by default", but grep shows no writer (always `Vec::new()` via `IndexDescriptor::new`) and no reader anywhere in the crate — it is only round-tripped by persistence and asserted in a compat test. It misleads API consumers into thinking backend tuning has a persisted channel (it does not — see 5.1).
- **Failure scenario:** an integrator relies on the documented tuning channel; nothing ever reads it.
- **Fix:** Either wire it into a backend (a natural home for the quantization mode of 5.2) or remove it before the format ossifies.

### 5.10 — low — Corrupt FTS posting values are silently replaced with `tf=1, doc_len=1`
- **File:line:** `crates/shamir-index/src/fts_ranked_backend.rs:125-130`.
- **Issue:** `bincode::deserialize(&val_bytes).unwrap_or(FtsPostingValue { tf: 1, doc_len: 1 })` conflates legacy MVP-era empty postings with genuine value corruption. A corrupted posting value silently yields wrong BM25 tf/doc_len inputs (skewed rankings) instead of surfacing an error or at least a log line — contrary to the workspace's "checksums everywhere / surface corruption" stance.
- **Failure scenario:** corrupted posting bytes silently skew BM25 rankings with no log, no counter, no error.
- **Fix:** Keep the empty-value fast path for legacy postings, but log at `warn` (or error) on a non-empty value that fails to decode.
- **Dedup:** also flagged as sub-item (a) of error-handling-lifecycle #10 — counted once here.

### 5.11 — nit — Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts
- **File:line:** `crates/shamir-index/src/lifecycle.rs:31-57` vs `crates/shamir-index/src/state.rs:59-73`; `crates/shamir-index/src/base_index/index_record_key.rs:62-81`, `:104-120`.
- **Issue:** lifecycle.rs still argues "Why no `Dropping` / `Failed` enum variant?" while `IndexState::Failed` shipped in R0-D (#1013) — the public lifecycle contract doc misdescribes the persisted enum. Separately, `IndexRecordKey::from_bytes` returns `Result<_, String>` (not a thiserror type) and silently accepts keys longer than 25 bytes; the deprecated `with_values` FxHasher helper remains fully `pub` "for tests only".
- **Failure scenario:** purely documentary/API-hygiene; misleads readers and leaves a test-only constructor at full public visibility.
- **Fix:** Update lifecycle.rs to describe `Failed` (link #1013); tighten `from_bytes` (length equality + proper error type) and demote `with_values` to `#[cfg(test)]` or remove it.

---

## 6. error-handling-lifecycle

The DDL error paths are unusually disciplined (enriched multi-phase messages, durable tombstones with rollback-on-persist-failure, RAII drain guards, documented fail-closed corruption policy F-83/#911, P0-4/#960). The findings below are where that discipline stops.

### 6.1 — high — `SortedIndexManager::load` swallows ALL store errors, silently loading zero sorted definitions
- **File:line:** `crates/shamir-index/src/base_index/sorted_index_manager.rs:2706-2709` — verified during synthesis (`Err(_) => return Ok(())`).
- **Issue:** The initial metadata read is `match self.info_store.get(sys_id...) { Ok(b) => b, Err(_) => return Ok(()) }`. The blanket `Err(_)` arm treats *every* store failure — IO errors, backend faults, corruption-class errors — identically to `NotFound`, i.e. "no sorted indexes exist". This directly contradicts the crate's own fail-closed policy: the sibling `IndexManager::new` (index_manager.rs:501-549) matches `DbError::NotFound` explicitly and propagates everything else, with a comment (F-83/#911) that even *claims* to "mirror `sorted_index_manager::load`'s corruption→`DbError::Codec` propagation" — that mirror covers only the *decode* path (which does propagate), not this store-*get* path.
- **Failure scenario:** a transient IO/backend error while opening a table causes the manager to load an empty definition set. The table opens "successfully" with all sorted indexes gone from planning; the next `persist_defs()` (any DDL on the table) writes the empty Vec back, permanently destroying every sorted-index definition while their postings remain as orphans.
- **Fix:** Match `Err(DbError::NotFound(_)) => return Ok(())` and propagate all other errors with `Err(e)`, mirroring `IndexManager::new`. Add a test injecting a failing store `get` (the `FaultyStore` pattern from `p12_ddl_partial_error_tests.rs`) proving the open aborts instead of returning an empty manager.

### 6.2 — high — Compaction double-write errors silently discarded; an incomplete graph is then swapped in as live
- **File:line:** `crates/shamir-index/src/vector/vector_backend.rs:296, 317, 354, 383, 512, 548` (swallows — the `let _ = target.adapter.upsert(...)` verified during synthesis); `vector_backend.rs:1101-1104` (unconditional swap).
- **Issue:** Every write mirrored to the compaction target discards its result: `let _ = target.adapter.upsert(rid, &v).await;` (and the `delete` / `apply_committed_vectors` twins). `run_background_compaction` never learns a double-write failed: Step 5 unconditionally `adapter_swap.store(...)`s the target in as the primary graph and Step 7 snapshots it. A failed target write means the post-compaction live graph is missing vectors/deletes — silently wrong ANN results — with **zero** log, error, or counter anywhere (this crate's own DDL paths follow a "do not swallow — log loudly" rule, e.g. index_manager.rs:2359-2372). The DELETE side of the same protocol is meticulously reconciled (`compaction_deleted_rids` + Step 4b) — only the UPSERT side is blind.
- **Failure scenario:** `target.adapter.upsert` returns `VectorError::Internal` (e.g. "f32 graph absent" class error, or a `spawn_blocking` join error) during the compaction window; the flip proceeds; the record's vector vanishes from similarity results until the next full rebuild — no signal at any layer.
- **Fix:** On a double-write error, set a shared `AtomicBool double_write_failed`; log loudly; have `run_background_compaction` check it before Step 5 and abort the flip (the compaction flag already clears via `CompactionFlightGuard`, so a retry happens on the next threshold crossing). At minimum, `log::warn!` every discarded error.
- **Dedup:** same defect flagged by concurrency-lockfree #5 (low) — counted once here.

### 6.3 — medium — `VectorBackend::drop_all` leaks the entire `__vec_snap__<id>` snapshot keyspace
- **File:line:** `crates/shamir-index/src/vector/vector_backend.rs:739-743` (`drop_all` no-op — verified during synthesis); `persistence.rs:531-548` (`sweep_index2_postings_by_id` scans only the 4-byte posting prefix); snapshot keys are string-keyed (`snapshot.rs:151-170`, `delta_chunk_key` 1150-1158).
- **Issue:** `VectorBackend::drop_all` is a documented no-op ("the graph lives in memory and dies with the process") — but since V2.1/V2.3 the graph IS persisted: snapshot chunks, sidecars, manifests and delta chunks under `__vec_snap__<id>` (vector_backend.rs:37). Nothing ever removes that keyspace on DROP (`sweep_index2_postings_by_id` scans the 4-byte LE id prefix, which cannot match the ASCII keyspace; grep-verified no other sweep exists), so every dropped vector index permanently leaks its full snapshot + delta log. The persistence.rs comment justifying the no-op predates the snapshot feature and is stale. If an id is ever recycled (`set_next_id` is caller-controlled), a fresh index would also resurrect a stale manifest/graph for the reused id.
- **Failure scenario:** create → snapshot → DROP INDEX → the entire `__vec_snap__<id>` keyspace survives as unbounded dead space; with id recycling, a new index on the same id loads a stale foreign graph.
- **Fix:** On drop (and in the tombstone recovery sweep), also `scan_prefix_stream` + `remove_many` the `__vec_snap__<id>` prefix; update the stale comment. Red test: create → insert → drop → assert zero keys under the snapshot keyspace.
- **Dedup:** same defect flagged by correctness-tdd #6 — counted once here.

### 6.4 — medium — index2 `drop_all` sweeps: per-key unbatched round-trips with errors swallowed, and FunctionalBackend buffers the whole index first
- **File:line:** `crates/shamir-index/src/fts_backend.rs:240-252` (`:248` swallow), `fts_ranked_backend.rs:401-413` (`:409` swallow), `functional_backend.rs:115-128 + 294-300` (`:298` swallow + full-index materialization at 294-300).
- **Issue:** All three storage-backed index2 backends sweep postings with `let _ = self.store.remove(key_bytes).await;` inside `drop_all`, then return `Ok(())`. Per lifecycle.rs and the R0-D contract, `drop_all` errors are supposed to be meaningful (caller leaves the tombstone / marks the backend `Failed`); the swallow makes those error paths dead code for these backends — a fully-failed sweep is reported as success, the DROP tombstone is cleared, and orphan postings remain with no signal. Independently, the per-key `remove` loop is O(N) sequential awaited round-trips (one fsync each on durable backends) — DROP INDEX on a 50M-posting FTS index → hours — and `FunctionalBackend::drop_all` first materializes ALL `(key, value)` pairs of the entire index into one unbounded `Vec` (peak RAM = whole index). The base_index family's `sweep_index_postings` (index_manager.rs:1243-1264, collect-then-one-`remove_many`) and `rekey_postings`' per-pass `transact` both demonstrate better shapes.
- **Failure scenario:** a DROP whose sweep fails halfway reports success, clears the tombstone, and leaves orphan postings forever; at scale the same sweep takes hours of sequential round-trips and, for the functional family, holds the entire index in RAM.
- **Fix:** Replace the per-key loops with collect-per-page + `remove_many(...).await?` (bounded batching fixes the round-trip and memory shape while making failures propagate); assert the enriched failure state in a `FaultyStore` test (pairs with 6.8).
- **Dedup:** the perf/memory aspect flagged by performance-hotpath #7 (med) and the sweep-materialization shape by concurrency-lockfree #6 (low) — one root cause (the unbatched, error-discarding drop_all sweep), counted once here.

### 6.5 — medium — `try_fit_and_rebuild` failures silently dropped behind comments that falsely claim logging
- **File:line:** `crates/shamir-index/src/vector/hnsw_adapter.rs:943-947, 2470-2475, 2709-2713`.
- **Issue:** All three trigger sites do `let _ = self.try_fit_and_rebuild().await;`. The inline comments state "Best-effort: a fit failure is logged inside `try_fit_and_rebuild` and does not fail the upsert" — but `try_fit_and_rebuild` contains **no logging at all** (grep for `log::` in hnsw_adapter.rs returns nothing); it returns `Err(VectorError::Internal(...))` which is then discarded. A failed fit (e.g. `spawn_blocking` join error, graph-build failure) leaves the adapter permanently on the f32 path — 4x memory and slower search — with no log, counter, or surfaced error, and the comments document observability that does not exist.
- **Failure scenario:** a one-off `spawn_blocking` panic during fit at a production deployment: SQ8 never activates for that adapter, memory stays 4x plan, and nothing in the logs explains why.
- **Fix:** `if let Err(e) = self.try_fit_and_rebuild().await { log::warn!("SQ8 fit failed, staying on f32 path: {e}"); }` at each site (or log inside the function and keep the `let _`), and correct the comments.

### 6.6 — medium — `build_index2_backend` panics via `unreachable!` on a persisted (disk-driven) descriptor kind
- **File:line:** `crates/shamir-index/src/build_backend.rs:66-68` — verified during synthesis.
- **Issue:** `IndexKind::Btree { .. } => unreachable!("Btree indexes are handled by the base_index index manager")`. The `desc` comes from `load_index2_metadata` — i.e. the on-disk `__meta__/indexes` blob — and nothing on the load path validates the kind. The function returns `Arc<dyn IndexBackend>` (not `Result`), so there is no way to fail closed. CLAUDE.md restricts panics to invariant violations meaning a *programmer* bug; a corrupted or future-versioned metadata blob containing a Btree kind is *data*, and the panic fires on the table-open path, contradicting the fail-closed corruption policy applied everywhere else (F-83, P0-4).
- **Failure scenario:** bit-flip or version-skew in the persisted descriptor blob yields a Btree kind → `TableManager::create` → panic at open, repeatedly, until manual repair — a persistent crash-loop rather than a typed error.
- **Fix:** Return `Result<Arc<dyn IndexBackend>, IndexError>` and map the arm to `IndexError::TypeMismatch` (fail this backend closed like `restore_on_open` errors), or filter/validate kinds in `decode_persisted_indexes`.
- **Dedup:** same defect flagged by security-crypto #8 (nit) — counted once here.

### 6.7 — medium — `IndexError` is stringly-typed; structured `DbError`s are flattened at every boundary
- **File:line:** `crates/shamir-index/src/backend.rs:56-66`; conversion sites at `write_ops.rs:93-97, 153-158`, `fts_backend.rs:102, 246`, `fts_ranked_backend.rs:121, 386, 407`, `functional_backend.rs:124`, `vector/vector_backend.rs:572`.
- **Issue:** `IndexError::Storage(String)` (and `Backend(String)`) receive `e.to_string()` of an underlying `DbError` everywhere, destroying the variant information. CLAUDE.md's error rule says "thiserror for library error enums (with `#[from]` where natural)" — there is no `#[from] DbError` and no way for a caller to distinguish NotFound / Io / `IndexDrainInProgress` / Corruption through the `IndexBackend` trait, all of which the same crate distinguishes carefully on its `DbResult`-returning APIs. Relatedly, `save_index2_metadata_with_pending` (persistence.rs:102-105) re-wraps the already-typed `DbError` from `Store::set` into `DbError::Internal(String)` — losing structure the sibling `save_legacy_index_version` and every tombstone writer propagate untouched (`?`).
- **Failure scenario:** a caller (including the crate's own 6.1 fix) cannot write `matches!(e, DbError::NotFound(_))`-style logic through the backend trait — the exact distinction 6.1 depends on.
- **Fix:** `Storage(#[from] shamir_storage::error::DbError)` (or a `Storage { source: DbError, ctx: String }` variant) and `?`/`#[from]` at the conversion sites; drop the `map_err` re-wrap in `save_index2_metadata_with_pending`.

### 6.8 — medium — Enriched error paths are unit-tested only for the regular-hash family
- **File:line:** `crates/shamir-index/src/base_index/tests/p12_ddl_partial_error_tests.rs` (only file with fault-injected error-path tests); untested paths in `index_manager_unique.rs:744-758, 839-875, 921-936`, `sorted_index_manager.rs:1003-1110, 1509-1605`, and the index2 backends.
- **Issue:** The P1-2/#967 fault-injection suite (`FaultyStore`) covers exactly three scenarios: regular-hash CREATE Phase-2 failure, Phase-3 failure, and DROP sweep failure. The same enriched messages for UNIQUE CREATE persist failure, UNIQUE DROP, SORTED DROP sweep/persist/tombstone-clear failures, SORTED RENAME definition-swap/rekey failures, and the index2 `drop_all` sweep are asserted nowhere in this crate (grep-verified: their distinctive strings appear in no test file; the engine crate doesn't test them either). `lifecycle.rs` (249-261) openly defers the full crash/cancellation matrix, but that deferral covers crash-window *state* tests — the cheaper "error text + partial state correct on injected failure" tests for the non-hash families are simply missing. Finding 6.1's swallow path (store-`get` error at sorted open) is likewise untested.
- **Failure scenario:** a refactor of any non-hash DDL error path silently breaks the enriched message or partial-state contract with a fully green suite.
- **Fix:** Extend `FaultyStore`-style injection to `create_unique_index_from_records`, `drop_unique_index`, `SortedIndexManager::drop_index`/`rename_index_sorted`, and one index2 backend `drop_all` (after fixing 6.4 so there is an error to observe); each asserting the enriched message and the post-failure durable state.

### 6.9 — low — `.unwrap()` on `SystemTime::duration_since(UNIX_EPOCH)` on four DDL success paths
- **File:line:** `index_manager.rs:2353-2355`, `index_manager_unique.rs:891-893`, `sorted_index_manager.rs:1070-1072, 1565-1567`.
- **Issue:** `completed_at` computation panics if the system clock is before the Unix epoch (CMOS reset, VM clock anomaly). The crate's own sibling code (descriptor.rs:48-51, meta_envelope.rs:38-41) uses `.unwrap_or(0)` for the exact same call — the four DDL sites are inconsistent with it.
- **Failure scenario:** a VM with a pre-epoch clock panics on every successful DDL completion.
- **Fix:** `.map(|d| d.as_millis() as u64).unwrap_or(0)` at all four sites.

### 6.10 — low — Read-hot-path `.expect` panics for the "quantized_active but unset" invariant, while sibling sites return errors
- **File:line:** `vector/hnsw_adapter.rs:1791, 1848, 1851, 1920, 1923, 2034, 2558, 2594`.
- **Issue:** The quantized search cores use `.expect("quantized_active but quantizer unset")` / `"...u8 graph unset"`, taking down the whole process on an invariant break. The mirror-image f32-path sites handle the same class of invariant break by returning `VectorError::Internal` ("NOT panic — upsert must propagate failure cleanly", hnsw_adapter.rs:2374-2378) or a defensive `Ok(vec![])` (search, 2913-2917). The Acquire-load memory-ordering argument for why the expects can't fire is documented, but if that argument is ever invalidated by a refactor, the failure mode on the query path is a server-killing panic rather than an `IndexError`.
- **Failure scenario:** a refactor breaks the documented ordering → every vector query panics the process instead of erroring.
- **Fix:** Convert the expects to `ok_or_else(|| VectorError::Internal(...))` for uniform failure behavior (they are not measurably hot relative to the graph traversal that follows).

### 6.11 — low — Silent degradation fallbacks with no logging (rebuild-skip + covering-projection residue)
- **File:line:** `fts_ranked_backend.rs:388-391` (undecodable record skipped in `rebuild`), `sorted_index_manager.rs:2861-2868` (`rmp_serde` failure → empty covering projection).
- **Issue:** These two paths choose a safe-but-degraded fallback with no observability: records that fail `InnerValue::from_bytes` are skipped during BM25 stats rebuild (stats drift from postings), and a msgpack encode failure silently empties a covering projection (per-row full-fetch fallback). Given the project's "checksums everywhere / log loudly" stance, these should at least log. (The bundle's other two sub-items — corrupt FTS posting value → default, and `FtsStats` underflow — are deduplicated to 5.10 and 2.8.)
- **Failure scenario:** BM25 stats silently drift from postings after one corrupt record; covering projections silently degrade to per-row fetches with no signal to explain a latency shift.
- **Fix:** `log::warn!` (rate-limited if hot) at each fallback.
- **Dedup:** sub-items of error-handling-lifecycle #10; sub-item (a) → 5.10, sub-item (d) → 2.8.

### 6.12 — nit — `IndexRegistry::insert` can still return `Err` leaving `by_id` populated (the exact partial publish #1009 closed via pre-check)
- **File:line:** `crates/shamir-index/src/registry.rs:288-307`.
- **Issue:** The #1009 fix pre-checks `by_name.contains_async` so the reachable name-collision path never touches `by_id` (tested). But the later `by_name.insert_async(...).map_err(...)?` arm still returns `Err` without rolling back the already-published `by_id` entry — the doc's own description of the pre-#1009 bug. Under the documented `ddl_admission` serialization this is unreachable (no concurrent same-name insert exists), but the error path itself is not self-cleaning.
- **Failure scenario:** none under current admission serialization; the path is a latent half-published-registry state.
- **Fix:** On that `Err`, `let _ = self.by_id.remove_async(&id).await;` before returning, or a `debug_assert` documenting the admission precondition.

### 6.13 — nit — Actor `shutdown` discards the join result (panic payload), and `BruteForceAdapter::shutdown` adds a lock-poisoning expect
- **File:line:** `actor.rs:100-105`; `vector/brute_force.rs:129-135`.
- **Issue:** `let _ = join.await;` drops the `JoinError`, so an applier task that panicked mid-op is indistinguishable from a clean drain — a silently-dead actor is only detectable later via `submit`'s `SendError`. `BruteForceAdapter::shutdown` additionally uses `.expect("brute-force join lock")` on a `std::sync::Mutex`, converting poisoning into a panic.
- **Failure scenario:** an applier panic during shutdown is invisible; a poisoned join mutex turns teardown into a panic.
- **Fix:** Log on `JoinError::is_panic` in both shutdowns; use `lock().unwrap_or_else(|p| p.into_inner())` for the join-handle mutex (the Option inside is still valid under poisoning).

---

## 7. style-claude-md

Broadly conformant: every `mod.rs` is a pure re-export manifest, tests live in dedicated per-module `tests/` directories, sanctioned `std::sync::Mutex` sites carry the required inline contention-model comments, and several files even cite the "one primary export per file" rule in their own docs. The two systemic deviations:

### 7.1 — medium — `use` statements inside function/block bodies across six production files
- **File:line:** `src/expr.rs:85-86`; `src/tokenizer.rs:306,321,464-465`; `src/write_ops.rs:165-166`; `src/base_index/index_manager.rs:1249,1443,1834,2794,2907`; `src/base_index/index_manager_unique.rs:578,656`; `src/vector/hnsw_adapter.rs:754,2327,2524`.
- **Issue:** CLAUDE.md ("📦 Imports at the top") mandates all `use` statements in the file header, with three documented exceptions (test-mod `use super::*;`, a commented trait-name collision, cfg-only imports). None of the 15 sites qualifies: every import (`futures::StreamExt` ×7, `scc::hash_map::Entry::{Occupied, Vacant}` ×3, `std::sync::OnceLock` ×2, `FxHasher`/`Hasher`, `ScalarError`, codec fns, `TFxMap`, `new_map`/`TMap`) hoists cleanly with no collision. `write_ops.rs:166` even re-imports `shamir_tx::IndexWriteOp` while the identical name is already `pub use`-exported at the top of the same file (`:9-10`) — the maintainability cost the rule guards against, already materialized. (`vector/simd.rs`'s `std::arch::*` imports are the legitimate cfg-gated exception; test files repeat the pattern in test fns.)
- **Failure scenario:** none at runtime; the drift normalizes the local-`use` habit for every new contributor.
- **Fix:** Hoist all listed imports to their file headers and delete the redundant `write_ops.rs:166` line. Land as a dedicated `style:` commit, `cargo fmt -p shamir-index` scoped to this crate.

### 7.2 — medium — Inline `#[cfg(test)] mod tests` inside an implementation file
- **File:line:** `src/vector/quant_meta.rs:83-111`.
- **Issue:** Verbatim violation of Test-organisation rule 5 ("Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files"). `quant_meta.rs` ends with an inline `mod tests` containing `quant_meta_round_trips_sq8_params` — the only instance in the crate; `vector/tests/` already exists with a manifest and sibling topic files that could host it. Ironic detail: the module's own doc (line 3) advertises "One primary export: [`QuantMeta`]".
- **Failure scenario:** none at runtime; it undermines an otherwise perfectly uniform layout and is the precedent the next inline test module will cite.
- **Fix:** Move the test to `vector/tests/quant_meta_tests.rs` (or fold into `quantization_snapshot_tests.rs`) and add `pub mod quant_meta_tests;` to `vector/tests/mod.rs`.
- **Dedup:** same defect flagged by correctness-tdd #14 (nit) — counted once here.

### 7.3 — low — `kind.rs` defines eight public types — "one file = one primary export" deviation
- **File:line:** `src/kind.rs:11-200` (`IndexKind`, `TokenizerKind`, `StemLanguage`, `FunctionalConfig`, `VectorMetric`, `VectorQuantization`, `VectorConfig`, `VectorBackendRef`).
- **Issue:** Eight public types is well past a "closely-coupled group," and several members have natural existing homes: `TokenizerKind`/`StemLanguage` belong beside `tokenizer.rs` (which currently imports them back — `tokenizer.rs:14`), and the four vector types belong under `vector/` (which imports `VectorMetric`/`VectorQuantization` back — `hnsw_adapter.rs:14`). Contrast `sq8.rs:50` and `quant_meta.rs:3` — the crate demonstrably holds itself to this rule elsewhere.
- **Failure scenario:** diffs touching tokenizer config and vector config collide in one file, weakening the atomic-diff / meaningful-`git blame` goal the rule states.
- **Fix:** Split (tokenizer kinds into `tokenizer.rs` or a `tokenizer_kind.rs`; the vector config family into `vector/config.rs`), keeping `kind.rs` re-exports for compatibility — a `style:`-scoped commit.

### 7.4 — low — Stale crate-root invariant: "NO `std::sync::Mutex` / `RwLock` / `parking_lot`"
- **File:line:** `src/lib.rs:8-11`.
- **Issue:** The crate-root doc lists as an architectural invariant: "**Lock-free**: … NO `std::sync::Mutex` / `RwLock` / `parking_lot`," while this same crate contains seven sanctioned `std::sync::Mutex` fields (`index_manager.rs:260,262,319,367,372`; `sorted_index_manager.rs:193,224`) — all sanctioned by CLAUDE.md's F-9/#1076 DDL-only exception, each with its required inline justification (verified). The invariant is arguably scoped to the index2 subsystem in context, but as written on the crate root it contradicts both the code beneath it and the workspace's own exception policy.
- **Failure scenario:** a reviewer treats the crate doc as authoritative and either re-litigates the sanctioned base_index Mutexes or cites the wrong rule to permit/reject a *new* Mutex.
- **Fix:** Reword to reference the policy: "lock-free on all read/write hot paths; `std::sync::Mutex` appears only under CLAUDE.md's sanctioned DDL-only/low-frequency exception categories, each justified inline."

### 7.5 — low — Feature-gated inline loom test module in an implementation file
- **File:line:** `src/reader_drain_gate.rs:306-422` (`#[cfg(loom)] mod loom_model`, `#[test]` at 389).
- **Issue:** Embeds a test module (with a `#[test]` fn) inside an implementation file. The letter of Test-org rule 5 targets `#[cfg(test)] mod tests`, and this module is a deliberately opt-in (cargo feature `loom`, compiled away from every normal build) model-checker harness with an unusually honest scope doc — a spirit-of-the-layout deviation, not a bright-line breach. The type's ordinary unit tests correctly live in `src/tests/reader_drain_gate_tests.rs`.
- **Failure scenario:** none at runtime; the risk is precedent.
- **Fix:** Add one sentence to the module doc acknowledging the tests/-layout deviation and why the model must sit beside the atomics it models, or relocate to a cfg(loom)-gated sibling under `tests/` if the feature plumbing permits.

### 7.6 — nit — Task-ID-prefixed test file names drift from topic-based naming
- **File:line:** `src/base_index/tests/`: `p03_drop_durability_tests.rs`, `p03b_sorted_drop_durability_tests.rs`, `p05b_sorted_rename_durability_tests.rs`, `p12_ddl_partial_error_tests.rs`, `f72_legacy_state_compat_tests.rs`, `p1068_ddl_op_log_retention_tests.rs`; also `index_manager_tests/{f72_,f78_,p1058_,p1098_,p1102_}*` and `sorted_index_manager_tests/{f71_,p1007_}*`.
- **Issue:** CLAUDE.md's test-organisation rule prescribes topic-named files; the `pNN`/`fNN` prefixes encode task provenance, not topic, so the directory's topic grouping degrades as tasks accumulate (already 12+ such files).
- **Failure scenario:** none.
- **Fix:** Prefer topic names for new test files; opportunistically fold task-named files into their topic homes when next touched (a `chore:`/`style:` commit, never riding a feature diff).

### 7.7 — nit — Comment nits: typo and a stale cross-file doc reference
- **File:line:** `src/reader_drain_gate.rs:113`; `src/base_index/index_definition.rs:48`.
- **Issue:** (a) `reader_drain_gate.rs:113` — "is not suffient proof" → "sufficient". (b) `index_definition.rs:48` cites "`index_write_op.rs::Provenance`" — no such file exists in this crate; `Provenance` lives in `shamir-tx` and is re-exported by `write_ops.rs:9-10`. The stale path sends a reader grepping this crate for a file that isn't there.
- **Failure scenario:** none.
- **Fix:** Fix the typo; repoint the doc reference to `crate::write_ops::Provenance` (or `shamir_tx::Provenance`).

### 7.8 — nit — Multi-type bundles in `backend.rs` and `bm25.rs` (borderline one-file-one-export)
- **File:line:** `src/backend.rs:20-66`; `src/bm25.rs:9-93`.
- **Issue:** `backend.rs` defines four public enums (`IndexQuery`, `FtsMode`, `IndexResult`, `IndexError`) alongside the `IndexBackend` trait; `bm25.rs` defines `Bm25Params`, `FtsPostingValue`, and `FtsStats` plus two free fns. Both are defensible as closely-coupled groups and much smaller outliers than `kind.rs` (7.3) — flagged only so the split decision is made consciously if 7.3 is acted on.
- **Failure scenario:** none.
- **Fix:** Optional: if `kind.rs` is split, consider giving `IndexQuery`/`IndexResult`/`IndexError` their own sibling files at the same time; otherwise leave as documented coupled groups.

---

## Finding counts

| Severity | Lens-tagged findings | Distinct defects after dedup | Distinct-defect numbers (dedup groups count once, at primary lens) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 10 | 10 | 1.1, 1.2, 1.3, 2.1, 4.1, 4.2, 5.1, 5.2, 6.1, 6.2 |
| medium | 27 | 25 | 1.4, 1.5, 1.6, 2.2, 3.1, 3.2, 3.3, 3.4, 4.3, 4.4, 4.5, 4.6, 4.7, 4.8, 5.3, 5.4, 5.5, 6.3, 6.4, 6.5, 6.6, 6.7, 6.8, 7.1, 7.2 |
| low | 29 | 24 | 1.7, 1.8, 1.9, 1.10, 1.11, 2.3, 2.4, 2.5, 3.5, 3.6, 4.9, 4.10, 4.11, 5.6, 5.7, 5.8, 5.9, 5.10, 6.9, 6.10, 6.11, 7.3, 7.4, 7.5 |
| nit | 13 | 11 | 2.6, 2.7, 2.8, 3.7, 4.12, 5.11, 6.12, 6.13, 7.6, 7.7, 7.8 |
| **total** | **79** | **70** | 10 high · 25 medium · 24 low · 11 nit |

Deduplicated census: **0 critical, 10 high, 25 medium, 24 low, 11 nit = 70 distinct defects** (79 lens-tagged findings).

Dedup ledger — 10 cross-lens dedup groups absorbing 10 lens-tagged findings (5.1 and 6.4 each absorb two), plus error-handling #3 split into two distinct defects (+1): 79 − 10 + 1 = 70.
1.5.1 ← correctness #8 + performance #11 · 6.2 ← concurrency #5 · 6.3 ← correctness #6 · 6.4 ← performance #7 + concurrency #6 · 5.7 ← error-handling #11 · 6.6 ← security #8 · 5.10 ← error-handling #10(a) · 2.8 ← error-handling #10(d) · 7.2 ← correctness #14 · 2.4 ← performance #12.

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Fix the functional-index hash collapse (1.1):** content-hash `Dec`/`Big`/`Bin` arms in `hash_inner` (mirroring `index_keys.rs`'s tag scheme) or fail closed at plan/index-creation time. Red test: distinct Dec/Big/Bin values → distinct posting keys. Closes **1.1**.
2. **Fix the tokenizer case-fold predicate (1.2):** Unicode-aware `!c.is_uppercase()` check in Whitespace + Full tokenizers. Red tests: `tokenize("Москва") == tokenize("москва")` for both tokenizers. Closes **1.2**.
3. **Stop swallowing store errors at sorted open (6.1):** match `DbError::NotFound` explicitly, propagate the rest; add a `FaultyStore` injected-`get`-failure test. Closes **6.1** (and its untested-path half of **6.8**).
4. **Delta-replay failure → full rebuild fallback (1.3):** replace warn-and-continue with `rebuild(data_store)`; Red test: corrupt one delta chunk → restart → `rebuild_count() == 1` and rows still findable after a snapshot flip. Closes **1.3**.
5. **Compaction double-write must not be blind (6.2):** record double-write failures (`double_write_failed` flag), check before the adapter swap, log loudly at minimum; a retry happens via `CompactionFlightGuard` on the next threshold crossing. Closes **6.2**.

**P1 — soon**
6. **Coherent vector snapshot dump (2.1):** quiesce promote-phase upserts/deletes for the dump (or a single write-barrier copy of the four maps); delete the stale "#402 will quiesce" comment. Closes **2.1**.
7. **Durable vector config + SQ8 carrier (5.1 + 5.2, one work item):** map `cfg.backend` → `HnswConfig` on the reopen path (error on `External`), persist the quantization mode (via `IndexDescriptor.options` (5.9) or a format-versioned carrier), restore it in `from_parts`, fix the `from_parts` doc; add a reopen test with non-default `ef_construct`/`m`. Closes **5.1, 5.2, 5.9**.
8. **Vector DROP/flip storage hygiene (6.3 + 5.5 + 6.4, one sweep work item):** implement `VectorBackend::drop_all` to sweep `__vec_snap__<id>`; extend `flip_generation` to prune old-gen q-chunks; convert all three index2 `drop_all`s to bounded collect-page → `remove_many(...)?`. Closes **6.3, 5.5, 6.4** (and the concurrency/perf flags deduped into 6.4).
9. **BM25 stats integrity (1.4 + 2.8 + 1.9):** guard `plan_update` Bumps like insert/delete; `saturating_sub` in `on_delete`; `sum == 0 → 1.0` clamp in `avg_doc_len`. Red tests per fix. Closes **1.4, 2.8, 1.9**.
10. **In-tx staged-vector dim validation (1.6) + posting-cache epoch re-check (1.5).** Closes **1.6, 1.5**.
11. **SQ8 observability + inline-fit latency (6.5 + 4.10):** log fit failures at the three trigger sites (correct the false comments) and move the fit to a single-flight background task. Closes **6.5, 4.10**.
12. **Fail-closed index2 builder (6.6):** `Result`-returning builder or kind validation in `decode_persisted_indexes` so a corrupt descriptor degrades instead of panic-looping at open. Closes **6.6**.
13. **Performance highs (4.1 + 4.2):** borrowing/Arc definition iteration in all planners; fold sorted/unique apply into one `Store::transact` mirroring the hash family. Closes **4.1, 4.2**.
14. **FTS lookup top-k bound (4.3):** stream-score into a bounded max-heap; stream-merge `AndAll` token scans. Closes **4.3**.
15. **Boundary hardening (3.4 + 3.3):** stop persisting the external API key value (secret reference / runtime resolution); enforce `is_indexable()` at this crate's eval dispatch. Closes **3.4, 3.3**.
16. **Expand fault-injection coverage (6.8 remainder):** UNIQUE CREATE/DROP, SORTED DROP/RENAME, one index2 `drop_all`. Closes **6.8**.

**P2 — backlog**
17. **Persisted-format hardening stream (3.1 + 3.2 + 5.4 + 5.3 + 5.8):** re-verify unique-index hits on values (then keyed/crypto digests for hash1/hash2 and 128-bit token identity), exact-pin `rustc-hash` + boot-time hash self-check folded into `LEGACY_INDEX_FORMAT_VERSION`, decide v1-snapshot compat (real shadow-shape decode or drop the claim + real v1-bytes test), ordinal-stability docs on `IndexKind`/`TokenizerKind`/`IndexExpr`. Closes **3.1, 3.2, 5.4, 5.3, 5.8**.
18. **Snapshot integrity (3.5 + 5.7 + 5.6):** sanitize `basename`/`qbasename`, CRC over manifest/sidecar payloads + fallible `to_quantizer`, header-first `MetaEnvelope`. Closes **3.5, 5.7, 5.6**.
19. **Remaining perf backlog:** per-row `Interner` (4.5), snapshot dump/load streaming + `mem::take` sidecar (4.6), delta-replay batching via `upsert_batch` (4.7), posting-cache byte budget (4.8), HNSW small-index zero-clone search (4.9), BruteForce publish cloning (4.11) and `spawn_blocking` search (2.5), commit-path backend map (4.12), `by_field_kind` reverse registry index once the uniqueness question is resolved (2.4), `drop_all` sweep batching remainder for base_index (via 6.4's pattern). Closes **4.5, 4.6, 4.7, 4.8, 4.9, 4.11, 2.5, 4.12, 2.4**.
20. **Error-typed cleanup (6.7 + 6.10 + 6.9 + 6.12 + 6.13):** `#[from] DbError` on `IndexError`, expect→`VectorError::Internal` on the quantized read path, `unwrap_or(0)` clock fallbacks, registry rollback on late name-collision, shutdown join logging + poisoning-tolerant lock. Closes **6.7, 6.10, 6.9, 6.12, 6.13**.
21. **Value-semantics leftovers:** order-independent Map/Set hashing in `hash_inner` (1.7), Dot-metric normalization enforcement (1.8), true successor bound for unbounded sorted ranges (1.10), explicit residual handling in `apply_index_ops_at_commit` (1.11), degradation-fallback logging (6.11), FTS n-gram caps (3.6), NEON aligned loads (3.7). Closes **1.7, 1.8, 1.10, 1.11, 6.11, 3.6, 3.7**.
22. **One style sweep commit (7.1 + 7.2 + 7.3 + 7.4 + 7.5 + 7.6 + 7.7):** hoist the 15 local `use`s, move the `quant_meta` inline test, split/reword per findings, topic-name new test files, fix the two comment nits. Closes **7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7**; 7.8 is a conscious-decision note, no action required.
