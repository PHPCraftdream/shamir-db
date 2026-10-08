<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-storage — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Concrete stale-fill, cursor and flag witnesses remain. Several registered tests have narrower discriminating power than their names/comments imply; no green-test or completion-last inference is justified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 9 | 0 | 0 | 3 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — MemBufferStore::get_many cache-fill can poison a Tombstone over a concurrent write — the #539 bug class survives in the vectored read

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

With K absent in inner, pause get_many after deciding None, acknowledge set(K,V), then complete the stale fill. Later wrapper get returns NotFound even after flush. No stale dirty_count assumption is required: get_many probes dirty directly before the writer.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1231); [crates/shamir-storage/src/storage_membuffer.rs:1258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1258); [crates/shamir-storage/src/storage_membuffer.rs:790](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L790).

<a id="review-2"></a>

### Claim 2 — MemBufferStore::transact post-commit cache republish clobbers a concurrent writer's fresher value — lasting stale read

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A differing writer injected during inner transact remains dirty, but transaction cache publication hides it. After drain, compare buffered.get and real_inner.get. Existing test asserts only the latter.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1048); [crates/shamir-storage/src/storage_membuffer.rs:1060](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1060); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:832](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L832).

<a id="review-3"></a>

### Claim 3 — InMemoryStore::iter_range_stream resumes inclusive + blind-skip instead of the mandated Bound::Excluded cursor

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Seed A,B,C; pull A with size one; remove A; next pull skips B and returns C. Existing batching test has no between-pull mutation. Exact scc supports excluded-bound ranges.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:196](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L196); [crates/shamir-storage/src/storage_in_memory.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L205); [crates/shamir-storage/src/tests/storage_in_memory_tests.rs:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_in_memory_tests.rs#L235).

<a id="review-4"></a>

### Claim 4 — MemBufferStore::remove / remove_many misreport the existed flag for keys resident only in inner

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Seed raw inner before constructing wrapper; remove/remove_many returns false while flush removes the value. Shared targets are wrapper-populated. Manager pre-read normally warms the cache, so default-server bookkeeping loss is not established by this simple witness.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:886](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L886); [crates/shamir-storage/src/tests/types_tests.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L97); [crates/shamir-engine/src/table/table_manager_crud.rs:456](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_crud.rs#L456).

<a id="review-5"></a>

### Claim 5 — TDD gap: CachedStore (both write modes) never runs the backend-agnostic batch contract suite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Helper callers remain only InMemory, MemBuffer and gated Fjall. Neither Cached mode invokes it; Mirrored dedicated assertions are partial coverage, not absence of all batch testing.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L38); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L33); [crates/shamir-storage/src/tests/storage_fjall_tests.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_fjall_tests.rs#L125).

<a id="review-6"></a>

### Claim 6 — FjallStore write-worker ordering claim holds only per handle-instance; Repo::store_get hands out a new instance per call

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each store_get constructs a new OnceLock; only insert/transact use it. Point writes are not universally ordered with transactions, and lazy worker teardown is explicitly joined rather than leaked.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L240); [crates/shamir-storage/src/storage_fjall.rs:312](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L312); [crates/shamir-storage/src/storage_fjall.rs:494](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L494).

<a id="review-7"></a>

### Claim 7 — InMemoryStore::set update path can resurrect an older value under concurrent same-key writers

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Remove/reinsert and ignored Duplicate remain, but an older overlapping write legally may linearize later. Retain the transient-absence facet; reject the later-completing-wins oracle.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L129); [crates/shamir-storage/src/storage_in_memory.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L130).

Grouping/duplicate: [concurrency-lockfree.md#3](concurrency-lockfree.md#review-3). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Nit — size-counter drift paths in CachedStore

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Insert always increments despite possible racing lazy-fill Duplicate. Reload separately clears/resets/refills while mutations update size. Assert final actual tree cardinality against the counter under controlled publication races.

Evidence: [crates/shamir-storage/src/storage_cached.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L311); [crates/shamir-storage/src/storage_cached.rs:423](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L423); [crates/shamir-storage/src/storage_cached.rs:480](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L480).

<a id="review-9"></a>

### Claim 9 — Nit — Repo::copy_store default has no from == to self-copy guard

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

copy_store calls set_many using original keys, never generated-key insert. Missing equality guard is real but the alleged doubling is contradicted by the implementation.

Evidence: [crates/shamir-storage/src/types.rs:488](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L488); [crates/shamir-storage/src/types.rs:500](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L500).

<a id="review-10"></a>

### Claim 10 — Nit — stale narration of retired mechanisms in #535 test/hook docs and mid-body imports

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Main hook header acknowledges the redesign, but batch hook/test comments still describe boolean restoration; function-local imports also remain. The seams are registered, not orphaned.

Evidence: [crates/shamir-storage/src/membuffer_clear_race_hook.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/membuffer_clear_race_hook.rs#L61); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:845](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L845); [crates/shamir-storage/src/types.rs:489](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L489).

<a id="review-nf-get-many-coverage"></a>

### Claim NF-get_many-coverage — Summary assertion that MemBuffer get_many lacks coverage

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The registered buffered shared-suite case asserts get_many hit/miss order and empty input after flush. It does not exercise concurrent stale fill, but positively refutes total absence.

Evidence: [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L28); [crates/shamir-storage/src/tests/types_tests.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L124).

<a id="review-nf-fjall-deleted-cursor"></a>

### Claim NF-fjall-deleted-cursor — Praised fjall deleted-cursor regression

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

k2 is removed before stream creation. With size two the first cursor is k3, so no emitted cursor is deleted between pulls. Output correctness does not establish the named counterfactual.

Evidence: [crates/shamir-storage/src/tests/storage_fjall_tests.rs:220](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_fjall_tests.rs#L220); [crates/shamir-storage/src/tests/storage_fjall_tests.rs:229](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_fjall_tests.rs#L229).

## Evidence and recipe corrections

- ConcurrentWriterInner::transact discards its original ops and only injects the writer. Removing guarded dirty cleanup would leave a miss, not the comment's transacted value; the asserted concurrent value still discriminates dirty-loss, not successful native transact behavior.
- Porting the single-get recheck cannot fully close a subsequent async cache.insert race; test durable backing and wrapper reads after all controlled operations settle.
- MemBuffer(Fjall) is not the hybrid data implementation. Default RepoInstance also unwraps the MVCC data store; practical exposure is strongest on buffered info/history and direct library configurations.
- The shared helper's bare is_err deletion checks do not identify NotFound specifically, and flag-forwarding mocks do not prove advertised atomicity.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage -- Correctness & TDD-coverage

## Summary
The crate's test culture is genuinely strong where it has been attacked (deterministic race seams for #535/#539, the F-41/F-49/F-59/F-77 `MirroredStore` atomicity suite, the fjall deleted-cursor regression, and the seven-category `KeyBytes` plan-doc suite), but the adversarial rigor is unevenly distributed: two variants of the exact tombstone/stale-cache poisoning race that #539 fixed in single-key `get()` survive in the *unfixed* siblings (`get_many`, `transact`'s cache republish) with no regression test covering them. One backend (`InMemoryStore`) structurally violates the scan-resume contract that `types.rs` declares every implementor MUST uphold, and a documented trait contract (`remove`'s existed flag) is misreported by `MemBufferStore` in a state the shared batch test cannot observe because its keys are always pre-dirtied. `CachedStore` — despite overriding several batched/scan ops' building blocks — never runs the shared backend-agnostic batch suite at all.

## Findings

### 1. `MemBufferStore::get_many` cache-fill can poison a Tombstone over a concurrent write — the #539 bug class survives in the vectored read
- **File:line:** `crates/shamir-storage/src/storage_membuffer.rs:1243-1260` (fill loop; ungated dirty probe at 1231), contrasted with the guarded single-key path at 840-880.
- **Severity:** high
- **Issue:** Single-key `get()` documents (#539 "tombstone-poisoning guard") that after an `inner.get()` round-trip it MUST re-check `dirty` before touching moka, because "moka gives no ordering between two independent tasks' inserts to the same key beyond last physical write wins" and a stale reader-inserted `Tombstone` landing after a writer's `Live` republish masks the write on every subsequent get "until evicted or overwritten — a LASTING mask" (default config has no TTL). `get_many` performs the identical miss → `inner.get_many` → `cache.insert(k, Slot::Tombstone)` sequence with **no dirty recheck at all** (lines 1252-1258 insert unconditionally).
- **Failure scenario:** Reader task R issues `get_many([K])`; R's probe of `dirty` misses (stale/invisible `dirty_count==0` per #539's accepted window). Writer task W completes `set(K, v)` (dirty insert + cache `Live` republish). R's slow `inner.get_many(K)` returns `None` (pre-flush); R then inserts `cache[K] = Tombstone` after W's republish. Every subsequent `get(K)` short-circuits on the cache-first branch and returns `NotFound` indefinitely, even after the flusher later lands `v` in inner — read-your-write broken permanently for that key until capacity eviction or another write.
- **Suggested fix:** Port the `get()` guard verbatim into the fill loop: before each `cache.insert(k, slot)`, re-check `dirty_count > 0 && dirty.get(&k).is_some()` and skip the fill when raced. Also add the deterministic-seam-style regression test (pause hook injected into the `inner.get_many` window) that this file's existing race tests model; there is currently no test covering `get_many` under concurrent writes.

### 2. `MemBufferStore::transact` post-commit cache republish clobbers a concurrent writer's fresher value — lasting stale read
- **File:line:** `crates/shamir-storage/src/storage_membuffer.rs:1043-1084` (esp. unconditional `cache.insert(k, Live(v))` at 1048 and `Tombstone` at 1071; the `remove_if` guard protects only `dirty`, not the cache).
- **Severity:** medium
- **Issue:** The audit §2.3 fix correctly guards `dirty` with `remove_if(slot == snapshot)`, so a concurrent `set` landing during `inner.transact` keeps its dirty entry. But the cache update is unconditional: this call re-inserts its own (now older) value into moka with no comparison against current state. By the module's own #539 reasoning, ordering between the two tasks' moka inserts is undefined, so the transact's stale value can land in the cache after the concurrent writer's fresh one. Reads hit cache-first, returning the pre-concurrent value; `dirty` holds the newer entry (so it will eventually reach inner), but the stale cache entry still wins every read until eviction/overwrite/TTL (None by default).
- **Failure scenario:** T1 runs `transact([Set K v1])`; during T1's I/O-length `inner.transact`, T2 completes `set(K, v2)` (dirty + cache = v2). T1's post-commit loop inserts `cache[K] = v1`. All subsequent `get(K)` return v1 although v2 was ACKed and sits in `dirty`.
- **Suggested fix:** Same family as finding 1 — guard the republish (skip when `dirty.get(&k)` holds a different slot than this op's value), or make the guard symmetric for both layers via one helper. Add a wrapper-injected concurrency test mirroring `transact_does_not_lose_concurrent_set` but asserting the READ side too (current test only checks what reaches `real_inner`).

### 3. `InMemoryStore::iter_range_stream` resumes inclusive + blind-skip instead of the mandated `Bound::Excluded` cursor — silent record drop under concurrent delete/update mid-scan
- **File:line:** `crates/shamir-storage/src/storage_in_memory.rs:184-231` (skip-first hack at 200-208, inclusive resume construction at 195-197); contract violated: `types.rs:316-336` ("each batch resumes strictly past the previous batch's last key (`Bound::Excluded`) — every implementor MUST uphold"), also contradicted by `src/README.md:366-369` which claims InMemoryStore uses the same Excluded pattern as CachedStore/Fjall.
- **Severity:** medium
- **Issue:** Batches ≥ 2 seek `range(resume..)` — INCLUSIVE of the previous batch's last key — and compensate by unconditionally skipping the first yielded item (`skip_first = !first_batch`). That equivalence only holds if the resume key still exists when the next batch queries. If it was removed between batches (a plain `remove`, or `set`'s own remove→insert update window at `storage_in_memory.rs:120-131`), the iterator starts at the successor S and the blind skip consumes S — one unseen record silently vanishes from the scan.
- **Failure scenario:** A range scan over posting/version keys with `batch_size < matches`; the batch-boundary key gets deleted or updated concurrently while the consumer awaits the next batch. The successor of that key is dropped from results — no error, wrong query output. Reachable through `MirroredStore` (primary IS InMemoryStore) and `MemBufferStore::iter_range_stream` delegating to an InMemoryStore inner.
- **Suggested fix:** Mirror the sibling implementations exactly: carry `last_key` and query `(Bound::Excluded(last), Unbounded)` like `storage_cached.rs:553-555` does against the same scc type — the tuple-bound form is already proven to compile/work in this workspace. Add a regression test interleaving a boundary-key removal between batch pulls.

### 4. `MemBufferStore::remove` / `remove_many` misreport the existed flag for keys resident only in `inner` — violates `Store::remove` contract, unobservable to the shared test
- **File:line:** `crates/shamir-storage/src/storage_membuffer.rs:884-894` (flag computed from dirty/cache only — no inner fallback), `1173-1191` (same for `remove_many`); contract: `types.rs:66`, `types.rs:168-171`; test gap: `types_tests.rs:97-104` (`run_batch_store_tests` remove section).
- **Severity:** medium
- **Issue:** `existed` is derived exclusively from `dirty` + cache. A key that exists durably in `inner` but has never been read/written through the buffer reports `false` while being removed — the opposite direction of failure versus `FjallStore::remove` (which does the real lookup) and `CachedStore::Sync` (which delegates). The shared batch suite never catches this because its removal targets always pass through `insert_many`/`set_many` first and are therefore always present in `dirty`/cache — the "clean-key removal" branch is vacuously uncovered for this backend. Per inline comments, engine callers consume these flags (`delete_returning_version` per `storage_fjall.rs:343-344`), so a "delete didn't exist" answer for a row that did exist is caller-visible. (`set`'s best-effort flag is explicitly documented at 763-768; `remove` carries only a pointer to that rationale and none of its justification applies — set's fallback errs toward the common case, remove's omission errs against reality.)
- **Failure scenario:** Engine opens hybrid table (`MemBuffer(Fjall)`); deletes row K never touched this session; buffer-layer delete returns `Ok(false)` though Fjall held and lost K; any logic branching on the flag (stats, conditional deletes, dedup) sees "nothing deleted".
- **Suggested fix:** Either fall back to the actual effect (`inner.remove(...)` result on full misses would double the op — better: consult a cheap existence check, or forward to `inner.remove_no_flag` + a `contains_key` equivalent via `get`), or amend the trait doc to mark MemBuffer's flag as advisory and add a red test proving the intended semantics so the divergence is at least deliberate.

### 5. TDD gap: `CachedStore` (both write modes) never runs the backend-agnostic batch contract suite
- **File:line:** `crates/shamir-storage/src/tests/types_tests.rs:38` (shared suite) vs. its three call sites only — `storage_in_memory_tests.rs:77`, `storage_membuffer_tests.rs:33`, `storage_fjall_tests.rs:125`; nothing in `storage_cached_tests.rs` invokes it.
- **Severity:** low
- **Issue:** The crate's central Red/Green instrument — `run_batch_store_tests` asserting `insert_many`/`set_many` flags, empty-input behavior, `get_many` order-and-None semantics, post-flush consistency, and `iter_range_stream_reverse` high→low ordering — excludes the one backend that overrides the very methods these defaults loop over (`set`/`remove` per-mode branches) and the mode (Async) whose whole point is deferred durability. CachedStore has 30+ dedicated tests, but the cross-backend invariant sweep — including "flags preserve input order" through the Async enqueue path and reverse-range default-impl composition — has never been asserted against it.
- **Failure scenario:** A future refactor of `CachedStore::set`'s Async branch (e.g. moving the `pending_writes.fetch_add`) breaks flag ordering or empty-batch contracts with no failing test.
- **Suggested fix:** Add `cached_sync_passes_full_batch_suite` / `cached_async_passes_full_batch_suite` over an in-memory inner; cheap, and instantiates findings 1/2's missing-race-test theme for another wrapper layer.

### 6. `FjallStore` write-worker ordering claim holds only per handle-instance; `Repo::store_get` hands out a new instance per call
- **File:line:** `crates/shamir-storage/src/storage_fjall.rs:240-244` (fresh `FjallStore` per `store_get`), 309-319 (`OnceLock` worker per instance), 494-495 ("ordered against every other point-write on this store"), 34-46 (`set`/`remove` bypass the worker by design).
- **Severity:** low
- **Issue:** Two handles to the same keyspace (engine refetches stores; e.g. `__tx__` marker store fetched per commit) each lazily spawn their OWN worker thread once they submit anything, so total order across `insert`/`transact` vs `set`/`remove` vs the other instance's worker does not exist — fjall's journal-writer mutex serializes execution but not intent order. Correctness is preserved (each op atomic, §B13 assumes no concurrent same-key writers anyway), but the comment overstates the guarantee, and DDL churn multiplies idle OS threads (one per store instance ever used for insert/transact).
- **Suggested fix:** Reword the invariant to scope it to a single handle (or share the worker per-keyspace via `Arc<Database>` + a name-keyed registry if the guarantee matters); document the thread-per-handle cost next to the lazy-spawn rationale.

### 7. `InMemoryStore::set` update path can resurrect an older value under concurrent same-key writers
- **File:line:** `crates/shamir-storage/src/storage_in_memory.rs:115-135` (insert-fail → `remove_sync` → re-`insert_sync` of the FIRST caller's original value).
- **Severity:** low
- **Issue:** Two concurrent `set(K, vA)` / `set(K, vB)`: A fails its insert, B completes remove+insert(vB), then A removes B's value and inserts vA — final state vA even though B finished logically later. Inline-documented as acceptable ("single-session in-memory backend"), and consistent with §B13's serialization assumption — recorded here only because the store is `Send + Sync` and fronts `MirroredStore`/test workloads where tooling may issue concurrent same-key writes without the engine's dispatch.
- **Suggested fix:** Leave as-is with the existing comment, or switch the fallback to CAS-style validation; no action forced.

### 8. Nit — size-counter drift paths in `CachedStore`
- **File:line:** `crates/shamir-storage/src/storage_cached.rs:420-424` (size incremented even if `insert_sync` rejects a duplicate — practically unreachable given fresh `RecordId`s), 306-327 (`reload()` clears the tree, stores `size = 0`, then repopulates non-atomically; concurrent writes during reload lose/double-count entries).
- **Severity:** nit
- **Issue/fix:** Increment only on `is_ok()` in `insert`; guard or document `reload` as diagnostic-only (it already says "useful if inner was modified externally", but engines calling it mid-flight would corrupt the O(1) counter mandated by the CLAUDE.md §O(x→0) pattern).

### 9. Nit — `Repo::copy_store` default has no `from == to` self-copy guard
- **File:line:** `crates/shamir-storage/src/types.rs:488-503`.
- **Severity:** nit
- **Issue:** Passing equal names streams the source onto itself (every record doubled; monotonic cursor prevents an infinite loop but not the corruption). RENAME TABLE passes distinct names today.
- **Suggested fix:** Early-return `DbError::Validation("copy_store: from == to")`.

### 10. Nit — stale narration of retired mechanisms in #535 test/hook docs and mid-body imports
- **File:line:** `crates/shamir-storage/src/membuffer_clear_race_hook.rs:1-19` and `storage_membuffer_tests.rs:845-866` still narrate the boolean sentinel's "store(false)" / "verify-after-clear restore" flow that #539's `dirty_count` redesign removed (the hook module itself admits the exercised interleaving "no longer reproduces a masked write"); discipline-wise, `use futures::StreamExt;` / `use std::ops::Bound;` appear mid-function/mid-closure at `types.rs:489`, `storage_cached.rs:218,307`, `storage_fjall.rs:451,610,673` contrary to CLAUDE.md's imports-at-top rule.
- **Severity:** nit
- **Suggested fix:** Trim the dead mechanism narrative to one line pointing at #539; hoist the imports (no collision exceptions apply).


</details>
