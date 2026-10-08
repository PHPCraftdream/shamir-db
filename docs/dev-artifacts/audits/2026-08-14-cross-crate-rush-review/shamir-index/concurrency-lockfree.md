<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-index — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Snapshot coordination and compaction cutover remain the main correctness concerns. Dirty-set and registry claims require production admission context. Structural scan costs are confirmed without latency measurements; scc is concurrent but not universally lock-free.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Background vector snapshot dumps the live adapter without quiescing; the multi-map sidecar scan is not atomic across maps (torn capture → permanent zombie graph node)

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Graph dump precedes independent forward/reverse/deleted/vector scans. The historical deleted-before-forward zombie schedule is impossible as written. A delayed graph promotion after an already-counted delta provides the reachable loss witness; coherency and applied watermark both require coordination.

Evidence: [crates/shamir-index/src/vector/snapshot.rs:529](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L529); [crates/shamir-index/src/vector/vector_backend.rs:919](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L919); [crates/shamir-engine/src/tx/commit_phases.rs:939](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit_phases.rs#L939).

<a id="review-2"></a>

### Claim 2 — `plan_records_created_batch` bypasses the in-flight-online-build dirty-set capture that every other write path performs

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Single-row creation diverts Building definitions to dirty capture; batch creation emits postings directly and engine batch callers reach it. This contradicts the routing invariant, but future batched update/delete corruption is not present-day proof.

Evidence: [crates/shamir-index/src/base_index/index_manager.rs:2458](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2458); [crates/shamir-index/src/base_index/index_manager.rs:2525](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2525); [crates/shamir-engine/src/table/table_manager_tx_ops.rs:836](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_tx_ops.rs#L836).

<a id="review-3"></a>

### Claim 3 — TOCTOU between lock-free `is_build_in_flight` check and Mutex-guarded dirty-set insert can leak an orphan dirty-set entry at Phase D

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The helper is check-then-insert, but supported Phase D raises admission, drains writers, and holds the write lock through final dirty drain and clear. The alleged production writer cannot overlap that clear. Direct precondition-breaking helper use is different.

Evidence: [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:2906](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L2906); [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:2947](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L2947); [crates/shamir-engine/src/table/table_manager.rs:1322](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager.rs#L1322).

<a id="review-4"></a>

### Claim 4 — `lease_by_field_and_kind` is an O(N) full-registry scan on every index2 read dispatch

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The by_id traversal remains documented design debt. Each descriptor is borrowed, not deep-cloned. A reverse registry needs a uniqueness/multiple-backend selection contract and synchronized create/drop/rename/state updates.

Evidence: [crates/shamir-index/src/registry.rs:609](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/registry.rs#L609); [crates/shamir-index/src/registry.rs:659](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/registry.rs#L659); [crates/shamir-index/src/registry.rs:666](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/registry.rs#L666).

<a id="review-5"></a>

### Claim 5 — Compaction double-write silently swallows upsert failures on the compaction target — a failed double-write becomes a permanent hole in the post-swap graph

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A failed target update after S0 capture is not repaired by insert-if-absent backfill. Result discards and unconditional swap persist the stale/missing target state. Error latching must also synchronize with late in-flight writes.

Evidence: [crates/shamir-index/src/vector/vector_backend.rs:296](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L296); [crates/shamir-index/src/vector/vector_backend.rs:512](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L512); [crates/shamir-index/src/vector/vector_backend.rs:1102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L1102).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — DROP-sweep paths materialize the entire index key list before removing (O(index-size) memory spike, one giant batch)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Regular and sorted sweeps buffer every key before remove_many. FTS instead removes individually. These are separate cold-path allocation/batching shapes; paged cleanup must preserve reader drain and durable tombstone ordering.

Evidence: [crates/shamir-index/src/base_index/index_manager.rs:1250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L1250); [crates/shamir-index/src/base_index/sorted_index_manager.rs:799](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/sorted_index_manager.rs#L799); [crates/shamir-index/src/fts_backend.rs:247](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_backend.rs#L247).

<a id="review-7"></a>

### Claim 7 — `BruteForceAdapter::search` runs an O(N·dim) exact scan inline on the async runtime (pillar 2: CPU-bound → `spawn_blocking`)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Distance scan and heap maintenance run inline. Public adapter injection permits library deployment, but default builder selects HNSW. No exact blocked-worker duration is established; moving owned snapshot work off-thread is compatible.

Evidence: [crates/shamir-index/src/vector/brute_force.rs:297](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/brute_force.rs#L297); [crates/shamir-index/src/build_backend.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/build_backend.rs#L53).

<a id="review-8"></a>

### Claim 8 — `ReaderDrainGate` doc invariant ("Never acquire any other lock while holding a `ReadGuard`") is contradicted in letter by `lookup_by_index`'s DashMap access inside the guard scope

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Cache access acquires shard locks while the read guard lives. Observed cache guards release before awaits and before destructive operations; this shows wording drift, not a lock-order cycle.

Evidence: [crates/shamir-index/src/reader_drain_gate.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/reader_drain_gate.rs#L85); [crates/shamir-index/src/base_index/index_manager.rs:2826](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2826); [crates/shamir-index/src/base_index/index_manager.rs:2874](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2874).

<a id="review-9"></a>

### Claim 9 — `BruteForceAdapter::join: std::sync::Mutex<Option<JoinHandle>>` lacks the inline contention-model comment CLAUDE.md requires per instance

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The private teardown slot still lacks an inline contention model. Its guard ends before join.await; runtime blocking/await-held mutex allegations are unsupported.

Evidence: [crates/shamir-index/src/vector/brute_force.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/brute_force.rs#L64); [crates/shamir-index/src/vector/brute_force.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/brute_force.rs#L131).

<a id="review-10"></a>

### Claim 10 — `FtsStats::on_delete` uses bare `fetch_sub` — underflow wraps to a huge `doc_count` with no saturating guard

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both fetch_sub operations wrap. Public invalid/double accounting can poison counts; empty→nonempty update supplies a transient spurious subtraction, but its paired addition normally cancels the count wrap before return. Saturation is defense, not correction of the underlying update bug.

Evidence: [crates/shamir-index/src/bm25.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/bm25.rs#L87); [crates/shamir-index/src/fts_ranked_backend.rs:223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L223).

## Evidence and recipe corrections

- Exact scc 3.8.4 published src/hash_map.rs documents bucket read-write serialization and warns about synchronous methods in async code. Existing concurrency design must not be advertised as formally lock-free merely because it uses scc.
- Correct snapshot chronology to forward map, reverse map, tombstones, vectors; preserve NEW.1 as the discriminating pruning-loss mechanism.
- Snapshot coordination needs one coherent graph/map capture and a contiguous applied boundary; reordering scans or quiescing only maps is insufficient.
- A four-byte NEON load cannot be replaced with an unconditional eight-byte vld1_u8 load under the existing four-byte loop bounds.
- ReaderDrainGate's loom model adds fences to its modeled operations and is opt-in; neither its existence nor historical green labels prove production execution.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-index -- Concurrency & lock-free invariants

## Summary

The crate is, on the whole, an exemplary implementation of CLAUDE.md's five pillars: `scc::HashMap` registries with `THasher`, `ArcSwap`/`NodeReplicated` RCU reads, atomic-mirror cardinality counters, a fully-atomics `ReaderDrainGate` with a worked SeqCst memory-model proof and an opt-in loom model, and every `scc::*::len()` call annotated with an O(N) ack. The banned patterns are absent where it matters: no `std::sync::Mutex` guard is held across an `.await` anywhere in the crate, and the DDL-only Mutexes (`dropping_*`, `renaming_*`, `dirty_sets`) each carry the inline "DDL op, contention is nil" comment the F-9/#1076 revision requires (and are the named sanctioned members of category 2). The findings below are the residual gaps: one high-severity non-atomic snapshot race in the vector background-snapshot path, an in-flight-build capture inconsistency in the batched write planner, two smaller TOCTOU/error-swallowing holes in the online-build and compaction protocols, and a few documented-debt / pillar-2 items.

## Findings

### 1. Background vector snapshot dumps the live adapter without quiescing; the multi-map sidecar scan is not atomic across maps (torn capture → permanent zombie graph node)

- **File:** `crates/shamir-index/src/vector/vector_backend.rs:891-934` (`run_background_snapshot`), triggered from `trigger_snapshot_check` (`:793-858`); scan site `crates/shamir-index/src/vector/snapshot.rs:522-549` (`dump_snapshot_with_gen`)
- **Severity:** high
- **Issue:** `run_background_snapshot` dumps the LIVE `HnswAdapter` while commit-Phase-5d upserts/deletes continue against it. `dump_snapshot_with_gen` first `file_dump`s the graph (seconds for a large index), then performs four INDEPENDENT `iter_sync` scans (`for_each_rid_map`, `for_each_rid_to_internal`, `for_each_deleted`, `for_each_vector`) at four different instants — there is no freeze, epoch, or copy that makes them one coherent snapshot. The code's own comment (`snapshot.rs:524-528`) justifies this with "the engine will gate them behind a quiesce in #402" — but #402 has landed and the background task it added does NOT quiesce; the assumption is stale. The delta-replay safety net does not close this hole: replay re-applies chunks with index ≥ the HWM captured at trigger time, which restores the *new* state of any mutated rid, but cannot remove a *stale* node that the torn capture kept live.
- **Failure scenario:** The snapshot fires exactly when writes are hot (it only triggers after `VECTOR_SNAPSHOT_DELTA_THRESHOLD` = 10k delta ops). A rid-replacing `upsert` (which atomically tombstones `old_internal` in `deleted` and swaps `rid_to_internal` under `entry_sync`, but inserts `rid_map[new_internal]` only later) lands between the `deleted` scan and the `rid_map` scan of the same dump. The persisted sidecar then contains `rid_map[old_internal] = rid` (rid_map entries are never removed) but NOT `deleted[old_internal]`. After restart, `load_snapshot` rebuilds a graph where `old_internal` is a live, unfiltered node resolving to the rid via `rid_map` — the rid surfaces TWICE in every top-k. Delta replay's `upsert(rid)` tombstones only the *current* `rid_to_internal` occupant, never the zombie; it persists until the next compaction. Silent wrong results, no error signal.
- **Suggested fix:** Make the dump a coherent point-in-time capture: either (a) quiesce Phase-5d promotes for the duration of the dump (the same serialization the stale comment assumed), or (b) copy all maps into owned `Vec`s under a single write-barrier/epoch (e.g. a promote-side seqlock or the existing `snapshot_in_flight` flag consulted by the promote path), or (c) at minimum re-order the scans so `deleted` is scanned LAST (captures the most tombstones) and document the residual window — (c) alone is not sufficient, it only narrows the window.

### 2. `plan_records_created_batch` bypasses the in-flight-online-build dirty-set capture that every other write path performs

- **File:** `crates/shamir-index/src/base_index/index_manager.rs:2512-2543` (batch planner; no `def.state == Building && is_build_in_flight` check), contrasted with the single-row planner at `:2447-2489` and `:2593-2604`; the violated invariant is documented at `:2019-2026`
- **Severity:** medium
- **Issue:** `apply_catchup_batch`'s doc asserts "every other write path routes through plan_record_created/updated/deleted, which correctly captures to the dirty-set while a build is in-flight." That is false for the batched planner: it emits direct `SetPosting` ops for EVERY definition, including a `Building` def whose build is registered in `in_flight_builds`. The engine drives both paths — single-row tx inserts through `plan_record_created`, batch tx inserts (`shamir-engine/src/table/table_manager_tx_ops.rs:809,1016`) and non-tx batch inserts (`table_manager_crud.rs:365`) through `plan_records_created_batch`. Today the divergence is mostly benign because inserts only create new rows (a direct write of a row absent from Phase A's pin is end-state-equivalent to capture-then-replay), but the documented invariant is broken, and the moment a batched update/delete planner is added (mirroring `plan_records_created_unique_batch`'s shape) it will silently skip the stale-posting removal that Phase C's pin-vs-current delta exists to perform — the exact "hidden divergence between two planner paths" bug class this crate's provenance/epoch machinery exists to prevent.
- **Suggested fix:** Hoist the `in_flight` check + dirty-set capture into `plan_records_created_batch` (same per-def predicate as `plan_record_created`), or make the single-row planner the only sanctioned path and route the batch wrapper through it. Either way, update the `apply_catchup_batch` doc to state the invariant in a way that cannot silently drift (CLAUDE.md F-1/#1027 already prescribes invariant-over-name-list for exactly this reason).

### 3. TOCTOU between lock-free `is_build_in_flight` check and Mutex-guarded dirty-set insert can leak an orphan dirty-set entry at Phase D

- **File:** `crates/shamir-index/src/base_index/index_manager.rs:1018-1032` (`get_or_create_dirty_set`) vs `:991-995` (`clear_build_in_flight`)
- **Severity:** low
- **Issue:** `get_or_create_dirty_set` checks `is_build_in_flight(name)` (lock-free `scc` read), then takes `dirty_sets`' outer `Mutex` and `entry().or_insert_with(...)` — two steps, not one atomic operation. A writer that passes the check just before Phase D's `clear_build_in_flight` removes both the registry entry and the dirty-set entry will re-create the map entry AFTER the removal. Since the build has finished, no Phase C drain ever runs again for that key: the `Arc<Mutex<BTreeSet<RecordId>>>` (and every RecordId subsequently inserted into it by racing writers until they observe the cleared registry) leaks for the manager's lifetime. The check and the insert cannot be made atomic without unifying the registry and the dirty-set under one structure — which is exactly the migration the field's own TODO (`:303-309`, "convert to `scc::HashMap`") already contemplates.
- **Suggested fix:** When executing the TODO, key the dirty-set as a value inside the same `scc::HashMap` entry as the in-flight marker so presence is atomic. Short of that, have `clear_build_in_flight` swap in a sentinel (or re-check `is_build_in_flight` under the outer `dirty_sets` lock held by the writer) so a late `or_insert_with` cannot resurrect a removed entry.

### 4. `lease_by_field_and_kind` is an O(N) full-registry scan on every index2 read dispatch

- **File:** `crates/shamir-index/src/registry.rs:644-683`
- **Severity:** low
- **Issue:** Every index2 query dispatch resolves its backend via `by_id.iter_async`, cloning and comparing `descriptor()` (including `paths`) for each registered backend until a match. This is the hottest read path the crate owns (it runs per query, under a held `ReadGuard` from `reader_gate.enter()` at `:650`), and it is O(number of backends on the table) rather than O(1). The #1091 investigation documented in the method doc is thorough and honestly states both why it is "not urgent at current per-table index counts" and what (the `(kind, field_path)` uniqueness question) blocks the reverse-index conversion — this is documented debt, not a hidden O(N), so it does not violate the letter of pillar 3, but it does violate its direction and should not be allowed to become permanent.
- **Suggested fix:** Resolve the fts/functional/btree `(kind, field_path)` uniqueness question (one-per-key vs multi-id) and land the `by_field_kind` reverse `scc::HashMap` sketched in the doc; until then, track the debt explicitly (e.g. in the method doc's header) so it is re-evaluated as per-table index counts grow.

### 5. Compaction double-write silently swallows upsert failures on the compaction target — a failed double-write becomes a permanent hole in the post-swap graph

- **File:** `crates/shamir-index/src/vector/vector_backend.rs:295-297` and `:316-318` (`plan_insert`/`plan_update`), `:511-513` (`apply_staged_vectors`); atomic swap at `:1102-1104`
- **Severity:** low
- **Issue:** The V4.2 double-write protocol meticulously reconciles the DELETE side (`compaction_deleted_rids` recorded before `target.adapter.delete`, then Step 4b reconcile) but the UPSERT side does `let _ = target.adapter.upsert(rid, &v).await;` — the `Result` is discarded with no log and no reconcile. If that upsert fails (e.g. a `spawn_blocking` join error / panic inside the hnsw insert), the compaction target is missing the vector, and once Step 5 atomically swaps the target in as primary, the vector is silently absent from all subsequent searches until the record is rewritten or a full rebuild/compaction runs. The primary-adapter upsert in the same functions propagates its error via `?`; only the shadow copy's failure is invisible.
- **Suggested fix:** At minimum `log::warn!` on double-write failure so the hole is observable; better, record failed rids in a reconcile set that Step 4b re-upserts from the old adapter's live set (the mirror image of the delete-side `compaction_deleted_rids` machinery).

### 6. DROP-sweep paths materialize the entire index key list before removing (O(index-size) memory spike, one giant batch)

- **File:** `crates/shamir-index/src/base_index/index_manager.rs:1243-1264` (`sweep_index_postings`); same shape at `crates/shamir-index/src/base_index/sorted_index_manager.rs:794-810` (`sweep_sorted_postings`); per-key `store.remove().await` loops at `crates/shamir-index/src/fts_backend.rs:240-252` and `fts_ranked_backend.rs:401-413` (`drop_all`)
- **Severity:** low
- **Issue:** `sweep_index_postings` streams the index's prefix in batches but accumulates EVERY key into `to_remove` before issuing one `remove_many` — peak memory and one transact batch are both O(total postings) for the dropped index. The sibling `rekey_postings` (`sorted_index_manager.rs:1359-1398`) already demonstrates the bounded alternative (one `transact` per scan pass, settle loop). The FTS `drop_all`s are the opposite extreme: one awaited `remove` round-trip per key. All are DDL/cold paths, so this is a pillar-3 "avoid hidden O(N) in helpers" polish item, not a hot-path violation.
- **Suggested fix:** Remove per scan batch (or per few batches) in `sweep_*_postings`, mirroring `rekey_postings`' per-pass `transact`; switch the FTS `drop_all`s to `remove_many` per batch.

### 7. `BruteForceAdapter::search` runs an O(N·dim) exact scan inline on the async runtime (pillar 2: CPU-bound → `spawn_blocking`)

- **File:** `crates/shamir-index/src/vector/brute_force.rs:262-325` (`search`, no `spawn_blocking`); `rebuild`'s per-page `upsert` loop also relies on the actor task's in-line processing
- **Severity:** low
- **Issue:** Every other CPU-heavy path in this crate (`HnswAdapter` graph traversals and inserts, `Sq8Quantizer::fit`, `file_dump`) is carefully pushed to `spawn_blocking`; `BruteForceAdapter::search` computes an exact distance for every stored vector directly on the tokio worker. `BruteForceAdapter` is documented as the baseline/test adapter, but it is reachable in production via `VectorBackend`'s adapter slot (any non-HNSW adapter), and its cost grows without bound with N — at 100k×128d that is tens of ms of blocked worker per query. Related: `upsert`'s `yield_now()` hack (`:249`) documents that write-then-read visibility is not guaranteed — acceptable for a baseline, but worth a doc line if the adapter is ever promoted.
- **Suggested fix:** Wrap the scan loop in `tokio::task::spawn_blocking` (the snapshot `Arc` already moves cleanly), matching `HnswAdapter::search`'s shape.

### 8. `ReaderDrainGate` doc invariant ("Never acquire any other lock while holding a `ReadGuard`") is contradicted in letter by `lookup_by_index`'s DashMap access inside the guard scope

- **File:** `crates/shamir-db` invariant stated at `crates/shamir-index/src/reader_drain_gate.rs:85-86`; DashMap shard-lock acquisitions inside the guard's scope at `crates/shamir-index/src/base_index/index_manager.rs:2826-2877` (`posting_cache.get`, `.iter().next()`, `.insert`)
- **Severity:** nit
- **Issue:** The gate's placement invariant is stated absolutely, but the sole production read chokepoint takes DashMap shard locks while holding the `ReadGuard`. This is safe in spirit — DashMap shard locks are leaf locks, never held across `.await`, and never held while acquiring the gate, so no lock-order cycle is constructible (and the `sweep_index_postings` drain runs only after `wait_for_drain` completes). But the absolute wording invites a future contributor to either "fix" the cache probe (pointless churn) or, worse, to treat the invariant as aspirational and acquire a real ordering hazard. Doc/code drift on a load-bearing concurrency contract is itself a hazard in this codebase's style.
- **Suggested fix:** Refine the invariant's wording to what is actually proved: "no lock that is (or can be) held while its holder waits on the gate or any DDL/admission lock may be acquired while holding a `ReadGuard` — the guard must remain the innermost lock in the DDL lock hierarchy." A comment at the `posting_cache` probe noting it satisfies this would close the loop.

### 9. `BruteForceAdapter::join: std::sync::Mutex<Option<JoinHandle>>` lacks the inline contention-model comment CLAUDE.md requires per instance

- **File:** `crates/shamir-index/src/vector/brute_force.rs:64` (field), `:129-135` (`shutdown`)
- **Severity:** nit
- **Issue:** The lock fits the sanctioned setup/teardown fallback class (locked exactly once, in `shutdown`, never on a hot path, never across `.await`), but CLAUDE.md's F-9/#1076 revision makes the inline comment naming the contention model the enforcement mechanism for every `std::sync::Mutex` on a runtime struct — precedent from another site is explicitly not sufficient. Every other Mutex in this crate carries the comment; this one is the outlier.
- **Suggested fix:** Add the one-line comment ("one-shot shutdown join-handle slot; locked once at teardown, contention nil") to keep the audit trail complete.

### 10. `FtsStats::on_delete` uses bare `fetch_sub` — underflow wraps to a huge `doc_count` with no saturating guard

- **File:** `crates/shamir-index/src/bm25.rs:87-92` (paired with `:80-85`)
- **Severity:** nit
- **Issue:** `doc_count`/`sum_doc_len` are two independent `AtomicU64`s updated with non-atomic pairs (fine for a derived BM25 average, which is approximate by design), but `on_delete`'s `fetch_sub` wraps silently on underflow. Any accounting bug that applies a `BumpFtsStats{sign: -1}` twice for one document (the double-count class `apply_index_ops_at_commit`'s provenance grouping exists to prevent) flips `doc_count` to ~2^64, and every subsequent `idf`/`avg_doc_len` becomes garbage with no error signal. The sibling `HnswAdapter::live_count` (`hnsw_adapter.rs:659-673`) already demonstrates the `saturating_sub` discipline for exactly this reason.
- **Suggested fix:** Use `fetch_update` with `saturating_sub` semantics (or clamp at 0) in `on_delete`, mirroring `live_count`'s documented stance that transient underflow must degrade to 0, never wrap.

</details>
