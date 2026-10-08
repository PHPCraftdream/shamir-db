<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-storage — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The concrete cache-ordering, non-atomic mutation, blocking-send, and eager-scan mechanisms remain. Claims about unavailable pinned dependency APIs are not independently verified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 8 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — CachedStore: unordered, non-atomic cache-mutation sites can leave the cache permanently behind inner

Status: `confirmed-open`. Current risk: `high`.

Sync backing writes and cache updates have independent ordering; cache_upsert and transact population still remove then insert and ignore Duplicate. A stale cache value can persist after all overlapping operations finish, disagreeing with inner. The concurrent test uses distinct writer keys.

Evidence: [crates/shamir-storage/src/storage_cached.rs:151](../../../../../crates/shamir-storage/src/storage_cached.rs#L151); [crates/shamir-storage/src/storage_cached.rs:403](../../../../../crates/shamir-storage/src/storage_cached.rs#L403); [crates/shamir-storage/src/storage_cached.rs:431](../../../../../crates/shamir-storage/src/storage_cached.rs#L431); [crates/shamir-storage/src/storage_cached.rs:471](../../../../../crates/shamir-storage/src/storage_cached.rs#L471); [crates/shamir-storage/src/tests/storage_cached_tests.rs:320](../../../../../crates/shamir-storage/src/tests/storage_cached_tests.rs#L320).

<a id="review-2"></a>

### Claim 2 — MemBufferStore::get_many is missing the #539 tombstone-poisoning guard its sibling get() has

Status: `confirmed-open`. Current risk: `high`.

The post-inner read-fill remains unconditional, unlike single-get's recheck. Ordinary shared-suite coverage exists, but no test controls this concurrent fill window.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:874](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L874); [crates/shamir-storage/src/storage_membuffer.rs:1246](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L1246); [crates/shamir-storage/src/storage_membuffer.rs:1258](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L1258); [crates/shamir-storage/src/tests/types_tests.rs:124](../../../../../crates/shamir-storage/src/tests/types_tests.rs#L124).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — InMemoryStore::set: remove+re-insert update path built on a false premise — swallows a racing Duplicate

Status: `confirmed-open`. Current risk: `medium`.

The remove/reinsert window and ignored duplicate result are present, allowing transient absence and an acknowledged call whose supplied value was not inserted. The proposed upsert API's existence/semantics are unverified against unavailable scc 3.8.4 sources; overlapping completion order alone is not a correctness oracle.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:124](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L124); [crates/shamir-storage/src/storage_in_memory.rs:129](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L129); [crates/shamir-storage/src/storage_in_memory.rs:130](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L130); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

<a id="review-4"></a>

### Claim 4 — FjallStore::submit blocks the tokio executor thread when the 1024-slot worker queue fills

Status: `confirmed-open`. Current risk: `medium`.

async submit directly invokes blocking SyncSender::send on a bounded channel before awaiting its reply. Saturation blocks executing runtime threads, not just tasks; timing and production saturation were not measured.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:92](../../../../../crates/shamir-storage/src/storage_fjall.rs#L92); [crates/shamir-storage/src/storage_fjall.rs:194](../../../../../crates/shamir-storage/src/storage_fjall.rs#L194); [crates/shamir-storage/src/storage_fjall.rs:199](../../../../../crates/shamir-storage/src/storage_fjall.rs#L199); [crates/shamir-storage/src/storage_fjall.rs:330](../../../../../crates/shamir-storage/src/storage_fjall.rs#L330); [crates/shamir-storage/src/storage_fjall.rs:496](../../../../../crates/shamir-storage/src/storage_fjall.rs#L496).

<a id="review-5"></a>

### Claim 5 — moka cache built with default RandomState instead of workspace THasher

Status: `confirmed-open`. Current risk: `low`.

The builder still uses plain build and never supplies THasher, unlike dirty. Exact default-hasher internals, build_with_hasher API support, and any speedup are unverified against the unavailable pinned moka source.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:142](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L142); [crates/shamir-storage/src/storage_membuffer.rs:232](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L232); [crates/shamir-storage/src/storage_membuffer.rs:255](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L255); [Cargo.lock:2221](../../../../../Cargo.lock#L2221).

<a id="review-6"></a>

### Claim 6 — InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole result set before first yield, under one pinned epoch Guard

Status: `confirmed-open`. Current risk: `medium`.

Both methods collect all matching key/value handles before constructing the stream, under one Guard. The Guard ends before yielding; peak result-vector storage remains proportional to all matches.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:153](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L153); [crates/shamir-storage/src/storage_in_memory.rs:154](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L154); [crates/shamir-storage/src/storage_in_memory.rs:158](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L158); [crates/shamir-storage/src/storage_in_memory.rs:240](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L240).

Grouping/duplicate: `performance-hotpath.md#2`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — CachedStore::reload is non-atomic clear→refill against live traffic

Status: `confirmed-open`. Current risk: `low`.

reload clears live cache and resets size before awaiting streamed refill, with no quiescence precondition or ordering guard. Concurrent writes/read-fills can disagree with refill and its counter. A cache hit does not fall through to inner, so reads are not universally safe.

Evidence: [crates/shamir-storage/src/storage_cached.rs:306](../../../../../crates/shamir-storage/src/storage_cached.rs#L306); [crates/shamir-storage/src/storage_cached.rs:311](../../../../../crates/shamir-storage/src/storage_cached.rs#L311); [crates/shamir-storage/src/storage_cached.rs:318](../../../../../crates/shamir-storage/src/storage_cached.rs#L318); [crates/shamir-storage/src/storage_cached.rs:471](../../../../../crates/shamir-storage/src/storage_cached.rs#L471).

<a id="review-8"></a>

### Claim 8 — NIT: cross-path write ordering inside FjallStore rests only on fjall's internal journal mutex arrival order

Status: `confirmed-open`. Current risk: `nit`.

Worker and spawn_blocking routes remain independent. The broad point-write-ordering comment is unsupported by those routes; dependency-internal journal locking was not independently verified.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:34](../../../../../crates/shamir-storage/src/storage_fjall.rs#L34); [crates/shamir-storage/src/storage_fjall.rs:337](../../../../../crates/shamir-storage/src/storage_fjall.rs#L337); [crates/shamir-storage/src/storage_fjall.rs:494](../../../../../crates/shamir-storage/src/storage_fjall.rs#L494); [crates/shamir-storage/src/storage_fjall.rs:550](../../../../../crates/shamir-storage/src/storage_fjall.rs#L550).

Grouping/duplicate: `correctness-tdd.md#6`. This row is not another independent defect.

<a id="review-nf-pillar-compliance"></a>

### Claim NF-pillar-compliance — Zero direct production Mutex/RwLock/parking_lot and no scc len calls; THasher, ArcSwap, atomic mirrors

Status: `not-applicable`. Current risk: —.

These source-level structural observations hold. They do not prove the entire backend stack is lock-free: DashMap is sharded-lock based, and dependency internals were not available.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:142](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L142); [crates/shamir-storage/src/storage_membuffer.rs:154](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L154); [crates/shamir-storage/src/storage_cached.rs:194](../../../../../crates/shamir-storage/src/storage_cached.rs#L194); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:765](../../../../../crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L765).

## Corrections and qualified non-findings

- The repository does not vendor the cited scc/moka sources; pinned external API claims must remain unverified.
- An atomic tree upsert alone cannot order backing commits against cache publication or prevent stale read-fill.
- Do not assert that a later-completing overlapping set must win.
- TableManager's conditional barrier/unique lock does not prove universal same-key serialization.
- The guard is held during collection, not across stream awaits.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage -- Concurrency & lock-free invariants

## Summary

Pillar compliance is strong: production code contains **zero** `std::sync::Mutex` / `RwLock` / `parking_lot` uses (only test-fixture Mutexes, correctly cloned-out before every `.await`), no `scc::*::len()` calls, `THasher` on the `dirty` DashMap, `ArcSwap` RCU for the moka hot-swap, and the atomic cardinality mirrors (`dirty_count`, `size`, `pending_writes`) carry exemplary Release/Acquire-pairing comments. The real findings are **semantic races in multi-step mutation sequences** (remove+insert upserts and unordered cache-fill sites that can silently lose an acked write or permanently mask a newer value with a stale cache/tombstone entry), one async-discipline gap (blocking channel send on an executor thread), and one pillar-4 gap (moka built without `THasher`). Two of these are races the codebase already fixed elsewhere (`get()`'s #539 tombstone-poisoning guard) but missed in a sibling path; none have dedicated test coverage.

## Findings

### 1. CachedStore: unordered, non-atomic cache-mutation sites can leave the cache permanently behind `inner` (silent acked-write loss, size-counter drift)

**File:line:** `src/storage_cached.rs:403-411` (`cache_upsert`), `427-467` (`set`), `469-485` (`get` lazy fill), `151-168` (`CacheAction::apply`), plus `306-327` (`reload`).

**Severity:** high

**Issue:** Every cache mutation is a two-step `remove_sync` + `insert_sync` pair, and there are three independent mutation sites (`cache_upsert`, lazy-fill `insert_sync`, transact-populate `apply`) with no shared ordering primitive between them. Consequences under concurrent same-key traffic:

1. **Silent lost update returning `Ok`.** In `WriteMode::Sync`, thread A runs `inner.set(K,vA)` then threads B/A race their `cache_upsert`s: A's `remove_sync(K)` succeeds, B's returns `false` (key transiently absent), B bumps `size` believing K new; B's `insert_sync(K,vB)` wins, A's re-insert hits `Duplicate` and is discarded by `let _ = self.cache.insert_sync(key, value)` — A's write vanishes from the cache while `inner` holds B. Whichever upsert lands last, not whichever committed to `inner` last, owns the cache.
2. **Persistent read-your-write violation.** Cache-update order is decoupled from `inner`-update order: A(`inner.set` vA) → B(`inner.set` vB) → B(`cache_upsert` vB) → A(`cache_upsert` vA) leaves cache=vA vs inner=vB **indefinitely** — every subsequent `get` serves the stale value (cache-first, line 471). Same shape via `transact`'s post-commit populate racing a standalone `set`.
3. **Counter drift:** each such interleaving bumps `size` for one logical entry; `cache_size()` telemetry creeps upward (correctness-neutral — nothing gates eviction on it).

No lock-free fix exists inside scc for "replace and keep order", but `TreeIndex::upsert_sync` (present in vendored scc 3.8.4, `tree_index.rs:393`) at least removes the removal window and the swallowed-Duplicate loss; #616 pt.2 already demonstrated the ordered-worker pattern for exactly this class of bug in Async mode.

**Failure scenario:** Engine calls through `BoxRepoFactory::cached(inner, Sync)` while an online index build and live writes touch the same posting keys; a writer gets `Ok(created=…)` but its value never becomes visible to readers until some later write or reload of that key.

**Suggested fix:** Route both Sync and Async-mode cache mutations through the single ordered worker (or collapse remove+insert into `upsert_sync` and make fill/populate go through one helper); accept-and-document remaining flag TOCTOU like Fjall §B13 does; derive `size` updates only inside that helper. Add a same-key concurrent-writer regression test (`test_cached_concurrent_access` currently uses 50 *distinct* keys, so this family is untested).

### 2. MemBufferStore::get_many is missing the #539 tombstone-poisoning guard its sibling get() has — stale reader cache-fill can mask a concurrent writer until eviction

**File:line:** `src/storage_membuffer.rs:1243-1261` (unguarded `cache.insert(k, slot)` after `inner.get_many`) vs `868-881` (the guarded single-key version), guard rationale documented at `817-867`.

**Severity:** high

**Issue:** Task #539 established (with an adversarial-review write-up) that a reader which falls through to `inner`, then fills the cache afterward, can land its stale entry *after* a concurrent writer's dirty-insert + cache republish — a LASTING mask that turns every later `get()` into a NotFound hit from cache. `get()` was fixed: immediately before caching it re-probes `dirty` (`raced = dirty_count > 0 && dirty.contains(key)`) and skips the fill on a race. `get_many` performs the identical reader sequence — miss probe, then one batched `inner.get_many().await` (a full slow I/O round-trip spanning all keys), then per-key `cache.insert(k, Slot::Live/Tombstone)` — with **no re-check at all**, so the window is not even narrowed to the single-call tail; it covers the entire backend round-trip for every key in the batch. This also affects writer Live values, not just tombstones.

Mitigating context (stated honestly): the file documents that full closure needs per-key serialization rejected on hot-path grounds, so a residual always remains in `get()` too — but `get_many` currently has the *large* pre-fix window, not the narrowed one. Additionally `get_many` has zero coverage in `storage_membuffer_tests.rs` (no mention of `get_many` anywhere in that suite).

**Failure scenario:** Vectored index/posting reads issue `get_many`; concurrently a tx commits a write to one of those keys between the dirty probe and the batched read's completion; the reader's stale Tombstone/Live lands post-republish; from then on every point `get` on that key returns the stale answer until moka evicts or TTL expires.

**Suggested fix:** Replicate the `raced` re-check before each cache-fill insert in `get_many` (re-probe `dirty` per key right before its `cache.insert`). Port `drain_clear_race_does_not_mask_acked_write`'s hook-based pattern into a `get_many` regression test.

### 3. InMemoryStore::set: remove+re-insert update path built on a false premise ("no update-in-place API") — swallows a racing Duplicate so a later-completing `set` can return Ok while its value never lands

**File:line:** `src/storage_in_memory.rs:115-135`; contradicting evidence: `src/storage_mirrored.rs:563-566` (lists `upsert_sync` among TreeIndex mutations) and vendored `scc-3.8.4/src/tree_index.rs:393` (`pub fn upsert_sync(&self, key: K, val: V)`); crate dep is `scc = "3.8"`.

**Severity:** medium

**Issue:** The comment claims "`scc::TreeIndex` has no update-in-place API so 2 traversals are unavoidable" — false for scc 3.8.x, which provides single-traversal `upsert_sync`. Beyond the cost claim being wrong, the Err branch (`remove_sync(&k); let _ = insert_sync(k, v)`) has a multi-writer interleaving where BOTH concurrent setters take the Err branch, A removes, B's remove no-ops, A inserts its value, and B's final `insert_sync` fails Duplicate — discarded silently — so **B completes `Ok(updated=true)` but its value never reaches the store** (A's, the earlier caller's, wins). This is distinct from (and worse than) the acknowledged "brief absence" window in lines 126-128. Also note sibling reviewer context: `MirroredStore::set`'s error-atomicity argument relies on primary writes being infallible, which holds, but nothing there orders two concurrent setters either.

Exposure is bounded by the same engine-level serialization Fjall §B13 leans on ("the engine never issues two concurrent `set` calls for the same key"), but unlike §B13 that acknowledgment is absent here and the safer primitive is one call away.

**Failure scenario:** Any non-engine/tooling caller issuing two overlapping `set`s on one hybrid-table key; the second call acks success yet restart-time hydration (which streams the primary/mirror) resurrects only the first value.

**Suggested fix:** Replace the Err branch with `self.data.upsert_sync(k, v)` (keep the `insert_sync` fast-path for the fresh-key case if desired); flag accuracy degrades to documented-best-effort (parity with Fjall/MemBuffer semantics), value-loss disappears. Correct the comment; add a two-task same-key set test asserting the completing-later write wins.

### 4. FjallStore::submit blocks the tokio executor thread when the 1024-slot worker queue fills

**File:line:** `src/storage_fjall.rs:92-93` (`sync_channel(1024)`), `188-201` (`tx.send(...)` called directly in `async fn submit`), call sites `330` (`insert`) and `496` (`transact`).

**Severity:** medium

**Issue:** Pillar 2 requires I/O-bound ops to be async/CPU-blocking work off the runtime. Here the enqueue onto the OS-thread worker is a synchronous `std::sync::mpsc::SyncSender::send` executed on the async caller's task. The comment says "a full queue simply parks the submitting task" — it actually **parks the runtime worker thread** running that task (blocking send, not task park): every other task scheduled on that core-thread stalls too. Under sustained commit pressure (batch commits are slow disk ops on the drain thread) ≥N_workers blocked submitters parked across different tokio workers removes the executor from service until the single fjall drain thread catches up — latency cliff/live-lock shape rather than backpressure (CLAUDE.md treats this class as bugs, not tuning). FIFO ordering and backpressure intent are sound; the wait mechanism is what violates the model.

**Failure scenario:** `transact`-heavy burst (commit pipeline fanning out beyond 1024 queued batches with fsync-bound commits); all tokio workers eventually sit inside blocking `send`s; unrelated timer/IO tasks on those workers stop firing until the drain drains.

**Suggested fix:** Keep the single OS-thread worker + sync_channel, but gate submission with a `tokio::sync::Semaphore` acquired via `.await` before the blocking `send` (preserves the no-extra-hop property and bounds in-flight submitters to the queue depth), or switch the front half to `tokio::sync::mpsc::bounded(1024)` with `.send().await`.

### 5. moka cache built with default `RandomState` instead of workspace `THasher` (pillar 4)

**File:line:** `src/storage_membuffer.rs:232-255` (`build_cache` ends in plain `.build()`; hasher import `THasher` already present at line 84 for the dirty map).

**Severity:** low

**Issue:** CLAUDE.md pillar 4 names `THasher` the default for every hash-keyed structure. `MemBufferStore` sits directly on the default disk stack (`BoxRepoFactory`: MemBuffer→Fjall), so every hot op hashes RecordKey twice+ (cache get + insert; weigher lookups) via SipHash-class hashing instead of FxHash. moka 0.12 supports this (`CacheBuilder::build_with_hasher`, verified in vendored moka 0.12.x builder docs); no API obstacle.

**Suggested fix:** `builder.build_with_hasher(shamir_collections::THasher::default())`.

### 6. InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole result set before first yield, under one pinned epoch Guard

**File:line:** `src/storage_in_memory.rs:147-169` (`iter_stream` full-map clone) and `234-257` (`scan_prefix_stream` collect-to-end).

**Severity:** low

**Issue:** Audit `2026-07-06-perf-radical-o-notation` §1.3 made incremental cursor streaming a correctness-of-doctrine item (implemented for CachedStore/Fjall; README advertises "constant memory per batch regardless of dataset size"). InMemoryStore still clones **every** matching `(key,value)` upfront — O(N)/O(matches) allocation before the first yield, full memory residency regardless of consumer appetite, and an `scc::Guard` pinned across the whole collect delays EBR reclamation of nodes removed concurrently during large scans. `iter_range_stream` in the same file shows the incremental resume pattern already; the doctrine applied inconsistently here. Historically significant consumers exist (MirroredStore primaries, MVCC/vacuum-era flows that motivated the TreeIndex migration).

**Suggested fix:** Reshape both streams to the per-batch bounded `range` + `Bound::Excluded(last_key)` resume loop already used by `iter_range_stream` (guard scoped per batch); align the README claim or scope it to Fjall/Cached.

### 7. CachedStore::reload is non-atomic clear→refill against live traffic

**File:line:** `src/storage_cached.rs:306-327`

**Severity:** low

**Issue:** `clear()` + `size.store(0)` followed by a streamed refill: concurrent Async-mode `cache_upsert`s can repopulate keys mid-refill whose values then lose the insert race against older streamed rows (stale refill overwrite of a fresher write persists until the next same-key write/reload); `size` accounting drifts while readers/writers interleave with the refill counter bumps. Reads are safe (fall-through to inner). Likely DDL-time-only usage today, hence low.

**Suggested fix:** Document reload as requiring quiescence (it is a resync/debug surface), or snapshot-diff into the live tree rather than clear-first.

### 8. NIT: cross-path write ordering inside FjallStore rests only on fjall's internal journal mutex arrival order

**File:line:** `src/storage_fjall.rs:337-389` (`set`), `550-580` (`remove`), `17-47` (worker rationale)

**Severity:** nit

**Issue:** Worker-routed ops (`insert`, `transact`, FIFO-submission-ordered) and spawn_blocking-routed ops (`set`/`set_no_flag`/`remove`/`remove_no_flag`, arbitrary pool scheduling) reach fjall in whatever order each path acquires the journal writer mutex — there is no cross-path ordering guarantee, i.e. structurally the same hazard #616 pt.2 eliminated inside CachedStore. The §B13 acknowledgment covers concurrent same-key `set`s via TableManager serialization, which also neutralizes this variant in practice, but the comment never names the worker-vs-spawn_blocking interleave explicitly; add one sentence when that block is next touched so a future routing change doesn't silently widen the assumption.

</details>
