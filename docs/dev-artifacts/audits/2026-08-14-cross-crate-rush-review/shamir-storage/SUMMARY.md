# shamir-storage — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate sweep)

Crate: `crates/shamir-storage/` — the storage spine: the `Store`/`Repo` KV abstraction
(`types.rs`), five backends/wrappers (`InMemoryStore`, `FjallStore` on the fjall LSM,
`MemBufferStore` write-back buffer + moka cache, `CachedStore` write-through/write-behind,
`MirroredStore` mirror-first hybrid), and the `KeyBytes` record-key type. Read-only
synthesis pass — no build/test/lint commands were run and no source file was modified.

Review basis: the seven 2026-08-14 lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and merged. Structure/tone/dedup conventions
calibrated on the two finished exemplars:
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. The raw
lens-tagged counts below (53) match this crate's row in the workspace
`SUMMARY.md` breakdown table (context only).

Dedup convention: where the same root-cause defect was flagged by multiple lenses, the
full write-up lives once under its primary lens; the other lenses carry a
`*(primary: X.Y)*` stub. A deduped defect's severity of record is the **highest**
severity any lens assigned to it (the range is noted in the entry). Spot-checks during
synthesis (`storage_membuffer.rs:1243-1260`, `storage_in_memory.rs:184-231`,
`storage_cached.rs:242`, `storage_fjall.rs:92-98`, `types.rs:9` vs `key_bytes.rs:1-9`)
confirmed the load-bearing file:line references; nothing new was found worth adding.

## Executive summary

The crate's foundations are unusually good for a storage layer — contract-grade trait
docs, pillar-clean concurrency primitives (`THasher` on `dirty`, ArcSwap hot-swap,
Release/Acquire mirror comments), deep race-regression suites where it has been attacked
(#539/#535, F-41/F-49/F-59/F-77), and zero `unsafe` — but it is **not shippable as-is**:
two unguarded cache-fill/republish races (`MemBufferStore::get_many` missing the proven
#539 tombstone guard; `CachedStore`'s three unordered remove+insert mutation sites) can
permanently mask acked writes, and `CachedStore::flush()` can park forever if the async
write-worker dies. Fix those three liveness/silent-loss defects first (P0 items 1–3),
then put versioning guardrails on the persisted `MemBufferConfig` blob before any further
schema churn (P0 item 4) — an unversioned change there breaks every existing database at
open. The O(range)-memory scan class (reverse streams, eager whole-corpus materialization
on InMemoryStore) is the top P1 theme.

---

## 1. correctness-tdd

### 1.1 — high — `MemBufferStore::get_many` cache-fill can poison a Tombstone over a concurrent write — the #539 bug class survives in the vectored read
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:1243-1260` (fill loop; ungated
  dirty probe at :1231), contrasted with the guarded single-key path at :840-880 (guard
  rationale documented at :817-867). *(primary: also flagged by concurrency-lockfree — §2.2)*
- Issue: Single-key `get()` documents (#539 "tombstone-poisoning guard") that after an
  `inner.get()` round-trip it MUST re-check `dirty` before touching moka, because moka gives
  no ordering between two independent tasks' inserts to the same key beyond last-physical-
  write-wins, and a stale reader-inserted `Tombstone` landing after a writer's `Live`
  republish masks the write on every subsequent get until evicted or overwritten — a LASTING
  mask (default config has no TTL). `get_many` performs the identical miss →
  `inner.get_many` → `cache.insert(k, Slot::Tombstone/Live)` sequence with **no dirty recheck
  at all** (:1252-1258 insert unconditionally — verified). Per the concurrency lens: the
  window is not even narrowed to the single-call tail as in fixed `get()` — it spans the
  entire batched backend round-trip for every key in the batch, and covers writer `Live`
  values, not just tombstones. Additionally `get_many` has zero coverage in
  `storage_membuffer_tests.rs` (no `get_many` test anywhere in that suite).
- Failure scenario: Reader task R issues `get_many([K])`; R's probe of `dirty` misses (stale/
  invisible `dirty_count==0` per #539's accepted window). Writer task W completes
  `set(K, v)` (dirty insert + cache `Live` republish). R's slow `inner.get_many(K)` returns
  `None` (pre-flush); R then inserts `cache[K] = Tombstone` after W's republish. Every
  subsequent `get(K)` short-circuits on the cache-first branch and returns `NotFound`
  indefinitely — read-your-write broken permanently for that key until capacity eviction or
  another write.
- Suggested fix: Port the `get()` guard verbatim into the fill loop: before each
  `cache.insert(k, slot)`, re-check `dirty_count > 0 && dirty.get(&k).is_some()` and skip the
  fill when raced. Port `drain_clear_race_does_not_mask_acked_write`'s hook-based pattern
  into a `get_many` regression test (pause hook injected into the `inner.get_many` window).

### 1.2 — medium — `MemBufferStore::transact` post-commit cache republish clobbers a concurrent writer's fresher value — lasting stale read
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:1043-1084` (esp. unconditional
  `cache.insert(k, Live(v))` at :1048 and `Tombstone` at :1071; the `remove_if` guard
  protects only `dirty`, not the cache).
- Issue: The audit §2.3 fix correctly guards `dirty` with `remove_if(slot == snapshot)`, so a
  concurrent `set` landing during `inner.transact` keeps its dirty entry. But the cache
  update is unconditional: the call re-inserts its own (now older) value into moka with no
  comparison against current state. By the module's own #539 reasoning, ordering between the
  two tasks' moka inserts is undefined, so the transact's stale value can land in the cache
  after the concurrent writer's fresh one. Reads hit cache-first and return the pre-concurrent
  value; `dirty` holds the newer entry (so it eventually reaches inner), but the stale cache
  entry wins every read until eviction/overwrite/TTL (None by default).
- Failure scenario: T1 runs `transact([Set K v1])`; during T1's I/O-length `inner.transact`,
  T2 completes `set(K, v2)` (dirty + cache = v2). T1's post-commit loop inserts
  `cache[K] = v1`. All subsequent `get(K)` return v1 although v2 was ACKed and sits in `dirty`.
- Suggested fix: Same family as 1.1 — guard the republish (skip when `dirty.get(&k)` holds a
  different slot than this op's value), or make the guard symmetric for both layers via one
  helper. Add a wrapper-injected concurrency test mirroring
  `transact_does_not_lose_concurrent_set` but asserting the READ side too (the current test
  only checks what reaches `real_inner`).

### 1.3 — medium — `InMemoryStore::iter_range_stream` resumes inclusive + blind-skip instead of the mandated `Bound::Excluded` cursor — silent record drop under concurrent delete/update mid-scan
- File:line: `crates/shamir-storage/src/storage_in_memory.rs:184-231` (skip-first hack at
  :200-208, inclusive resume construction at :195-197 — verified); contract violated:
  `types.rs:316-336` ("each batch resumes strictly past the previous batch's last key
  (`Bound::Excluded`) — every implementor MUST uphold"); also contradicted by
  `src/README.md:366-369`, which claims InMemoryStore uses the same Excluded pattern as
  CachedStore/Fjall.
- Issue: Batches ≥ 2 seek `range(resume..)` — INCLUSIVE of the previous batch's last key —
  and compensate by unconditionally skipping the first yielded item
  (`skip_first = !first_batch`). That equivalence only holds if the resume key still exists
  when the next batch queries. If it was removed between batches (a plain `remove`, or
  `set`'s own remove→insert update window at `storage_in_memory.rs:120-131`), the iterator
  starts at the successor S and the blind skip consumes S — one unseen record silently
  vanishes from the scan. (Interaction note for 4.1: this same body is what the perf lens
  proposes as the template for the missing reverse-stream overrides — fix the resume
  discipline first so the template is sound.)
- Failure scenario: A range scan over posting/version keys with `batch_size < matches`; the
  batch-boundary key is deleted or updated concurrently while the consumer awaits the next
  batch. The successor of that key is dropped from results — no error, wrong query output.
  Reachable through `MirroredStore` (primary IS InMemoryStore) and
  `MemBufferStore::iter_range_stream` delegating to an InMemoryStore inner.
- Suggested fix: Mirror the sibling implementations exactly: carry `last_key` and query
  `(Bound::Excluded(last), Unbounded)` like `storage_cached.rs:553-555` does against the same
  scc type. Add a regression test interleaving a boundary-key removal between batch pulls.

### 1.4 — medium — `MemBufferStore::remove`/`remove_many` misreport the existed flag for keys resident only in `inner` — violates the `Store::remove` contract, unobservable to the shared test
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:884-894` (flag computed from
  dirty/cache only — no inner fallback), :1173-1191 (same for `remove_many`); contract:
  `types.rs:66`, `types.rs:168-171`; test gap: `types_tests.rs:97-104`
  (`run_batch_store_tests` remove section). Related API-level framing: §5.3 (distinct
  disclosure issue, not deduped).
- Issue: `existed` is derived exclusively from `dirty` + cache. A key that exists durably in
  `inner` but has never been read/written through the buffer reports `false` while being
  removed — the opposite direction of failure versus `FjallStore::remove` (which does the
  real lookup) and `CachedStore::Sync` (which delegates). The shared batch suite never
  catches this because its removal targets always pass through `insert_many`/`set_many`
  first and are therefore always present in dirty/cache — the "clean-key removal" branch is
  vacuously uncovered for this backend. Per inline comments, engine callers consume these
  flags (`delete_returning_version` per `storage_fjall.rs:343-344`), so a "delete didn't
  exist" answer for a row that did exist is caller-visible. (`set`'s best-effort flag is
  explicitly documented at :763-768; `remove` carries only a pointer to that rationale, and
  none of its justification applies — set's fallback errs toward the common case, remove's
  omission errs against reality.)
- Failure scenario: Engine opens a hybrid table (`MemBuffer(Fjall)`); deletes row K never
  touched this session; the buffer-layer delete returns `Ok(false)` though Fjall held and
  lost K; any logic branching on the flag (stats, conditional deletes, dedup) sees "nothing
  deleted".
- Suggested fix: Either fall back to the actual effect (consult a cheap existence check, or
  forward to `inner.remove_no_flag` + a `contains_key` equivalent via `get`), or amend the
  trait doc to mark MemBuffer's flag as advisory and add a red test proving the intended
  semantics so the divergence is at least deliberate.

### 1.5 — low — TDD gap: `CachedStore` (and `MirroredStore`) never run the backend-agnostic batch contract suite
- File:line: suite at `crates/shamir-storage/src/tests/types_tests.rs:38`
  (`run_batch_store_tests`); call sites only `storage_in_memory_tests.rs:77`,
  `storage_membuffer_tests.rs:33`, `storage_fjall_tests.rs:125`; nothing in
  `storage_cached_tests.rs` (33 tests) or `storage_mirrored_tests.rs` (18 tests) invokes it.
  *(primary: also flagged by api-wire-protocol §5.6 and style-claude-md §7.5)*
- Issue: The crate's central Red/Green instrument — asserting `insert_many`/`set_many`
  flags, empty-input behavior, `get_many` order-and-None semantics, post-flush consistency,
  and `iter_range_stream_reverse` high→low ordering — excludes the two backends that most
  need it: `CachedStore` overrides the very methods these defaults loop over (`set`/`remove`
  per-mode branches) and has the mode (Async) whose whole point is deferred durability;
  `MirroredStore` inherits defaults through delegation (per the api lens; its bespoke suites
  cover mirror atomicity but not the common batch contract). The cross-backend invariant
  sweep — including "flags preserve input order" through the Async enqueue path and
  reverse-range default-impl composition — has never been asserted against either.
- Failure scenario: A future refactor of `CachedStore::set`'s Async branch (e.g. moving the
  `pending_writes.fetch_add`) or a delegation-path regression in either wrapper breaks flag
  ordering, empty-batch, or reverse-order contracts with no failing test — the safety net
  designed to catch precisely that does not exercise them.
- Suggested fix: Add `cached_sync_passes_full_batch_suite` /
  `cached_async_passes_full_batch_suite` over an in-memory inner, and (with a toy
  classifier) a `MirroredStore` over `InMemoryRepo` doing the same; keep Fjall/MemBuffer
  coverage as-is.

### 1.6 — low — `FjallStore` write-ordering claim holds only per handle-instance; `Repo::store_get` hands out a new instance per call (each lazily spawns its own OS worker)
- File:line: `crates/shamir-storage/src/storage_fjall.rs:240-244` (fresh `FjallStore` per
  `store_get`), :309-319 (`OnceLock` worker per instance), :494-495 ("ordered against every
  other point-write on this store"), :34-46 (`set`/`remove` bypass the worker by design).
  *(primary: also flagged by concurrency-lockfree §2.8 — the worker-vs-`spawn_blocking`
  cross-path interleave — and error-handling-lifecycle §6.7 — the per-instance worker
  lifecycle/churn facet)*
- Issue: Three facets of one structural fact. (a) Ordering (correctness lens): two handles to
  the same keyspace (the engine refetches stores; e.g. the `__tx__` marker store per commit)
  each lazily spawn their OWN worker once they submit anything, so total order across
  `insert`/`transact` vs `set`/`remove` vs the other instance's worker does not exist —
  fjall's journal-writer mutex serializes execution but not intent order; the :494-495
  comment overstates the guarantee. (b) Cross-path interleave (concurrency lens): even within
  one handle, worker-routed ops (FIFO-submission-ordered) and `spawn_blocking`-routed ops
  (`set`/`set_no_flag`/`remove`/`remove_no_flag`, arbitrary pool scheduling) reach fjall in
  whatever order each path acquires the journal mutex — the comment never names this
  interleave; §B13's TableManager serialization neutralizes it in practice, but only by
  prose. (c) Lifecycle (error lens): unlike `InMemoryRepo` (which caches `Arc<dyn Store>`
  per name in a `TDashMap`), every `store_get` builds a new `FjallStore` with a fresh
  `OnceLock`; the design leans entirely on the documented convention that short-lived
  instances must never issue `insert`/`transact`, else each call spawns an OS-thread write
  worker just to abandon it (spawn+join churn per transaction); outstanding handles also keep
  operating on a keyspace after a concurrent `store_delete` (backend-dependent errors).
  Correctness is preserved today (each op atomic; §B13 assumes no concurrent same-key
  writers) — all three lenses rate this low/nit.
- Failure scenario: A future commit path submits one `insert` through the per-commit marker
  store → silent OS-thread create/join storm on the hot path (invisible until
  bench/flamegraph); DDL churn multiplies idle threads (one per store instance ever used for
  insert/transact); and the documented "ordered against every other point-write" invariant is
  silently false across handles, so a routing change can widen the race unnoticed.
- Suggested fix: Reword the invariant to scope it to a single handle and name the
  worker-vs-`spawn_blocking` interleave in the §B13 block; share the worker per-keyspace via
  `Arc<Database>` + a name-keyed `Arc` registry (mirroring `InMemoryRepo`) if the guarantee
  matters — that also removes the churn hazard. Document the thread-per-handle cost next to
  the lazy-spawn rationale.

### 1.7 — low *(as tagged here)* — *(primary: 2.3)* — `InMemoryStore::set` update path can resurrect an older value under concurrent same-key writers
- The concurrency lens carries the full write-up (it rated the same defect **medium** — the
  severity of record; the correctness lens rated it low, noting the inline "single-session
  in-memory backend" acknowledgment and §B13 consistency).

### 1.8 — nit — `CachedStore` size-counter drift: increment on rejected duplicate insert
- File:line: `crates/shamir-storage/src/storage_cached.rs:420-424` (size incremented even if
  `insert_sync` rejects a duplicate — practically unreachable given fresh `RecordId`s).
- Issue/fix: Increment only on `is_ok()` in `insert`; guard the O(1) counter mandated by
  CLAUDE.md §O(x→0). (The finding's other half — non-atomic `reload()` at :306-327 — is the
  same defect as §2.7 and is counted once there.)

### 1.9 — nit — `Repo::copy_store` default has no `from == to` self-copy guard
- File:line: `crates/shamir-storage/src/types.rs:488-503`. Related (distinct defect, not
  deduped): §6.6 partial-failure orphaning on the same method.
- Issue: Passing equal names streams the source onto itself (every record doubled; the
  monotonic cursor prevents an infinite loop but not the corruption). RENAME TABLE passes
  distinct names today.
- Suggested fix: Early-return `DbError::Validation("copy_store: from == to")`.

### 1.10 — nit — Stale narration of the retired #535 mechanism in race-hook docs
- File:line: `crates/shamir-storage/src/membuffer_clear_race_hook.rs:1-19` and
  `storage_membuffer_tests.rs:845-866` still narrate the boolean sentinel's "store(false)" /
  "verify-after-clear restore" flow that #539's `dirty_count` redesign removed (the hook
  module itself admits the exercised interleaving "no longer reproduces a masked write").
  (The finding's other half — mid-body `use` statements — is the same defect as §7.1 and is
  counted once there.)
- Suggested fix: Trim the dead mechanism narrative to one line pointing at #539.

---

## 2. concurrency-lockfree

Pillar verdict (from the lens, kept for calibration parity): production code contains zero
`std::sync::Mutex`/`RwLock`/`parking_lot` (only test-fixture Mutexes, correctly cloned-out
before every `.await`), no `scc::*::len()`, `THasher` on the `dirty` DashMap, ArcSwap RCU
for the moka hot-swap, and exemplary Release/Acquire-pairing comments on the atomic
cardinality mirrors. The real findings are semantic races in multi-step mutation sequences.

### 2.1 — high — CachedStore: unordered, non-atomic cache-mutation sites can leave the cache permanently behind `inner` (silent acked-write loss, size-counter drift)
- File:line: `crates/shamir-storage/src/storage_cached.rs:403-411` (`cache_upsert`),
  :427-467 (`set`), :469-485 (`get` lazy fill), :151-168 (`CacheAction::apply`), plus
  :306-327 (`reload`).
- Issue: Every cache mutation is a two-step `remove_sync` + `insert_sync` pair, and there are
  three independent mutation sites (`cache_upsert`, lazy-fill `insert_sync`, transact-
  populate `apply`) with no shared ordering primitive between them. Consequences under
  concurrent same-key traffic:
  1. **Silent lost update returning `Ok`.** In `WriteMode::Sync`, thread A runs
     `inner.set(K,vA)` then threads B/A race their `cache_upsert`s: A's `remove_sync(K)`
     succeeds, B's returns `false` (key transiently absent), B bumps `size` believing K new;
     B's `insert_sync(K,vB)` wins, A's re-insert hits `Duplicate` and is discarded by
     `let _ = self.cache.insert_sync(key, value)` — A's write vanishes from the cache while
     `inner` holds B. Whichever upsert lands last, not whichever committed to `inner` last,
     owns the cache.
  2. **Persistent read-your-write violation.** Cache-update order is decoupled from
     `inner`-update order: A(`inner.set` vA) → B(`inner.set` vB) → B(`cache_upsert` vB) →
     A(`cache_upsert` vA) leaves cache=vA vs inner=vB **indefinitely** — every subsequent
     `get` serves the stale value (cache-first, :471). Same shape via `transact`'s
     post-commit populate racing a standalone `set`.
  3. **Counter drift:** each such interleaving bumps `size` for one logical entry;
     `cache_size()` telemetry creeps upward (correctness-neutral — nothing gates eviction on
     it).
  No lock-free fix exists inside scc for "replace and keep order", but
  `TreeIndex::upsert_sync` (present in vendored scc 3.8.4, `tree_index.rs:393`) at least
  removes the removal window and the swallowed-Duplicate loss; #616 pt.2 already
  demonstrated the ordered-worker pattern for exactly this class of bug in Async mode.
- Failure scenario: Engine calls through `BoxRepoFactory::cached(inner, Sync)` while an
  online index build and live writes touch the same posting keys; a writer gets
  `Ok(created=…)` but its value never becomes visible to readers until some later write or
  reload of that key.
- Suggested fix: Route both Sync and Async-mode cache mutations through the single ordered
  worker (or collapse remove+insert into `upsert_sync` and make fill/populate go through one
  helper); accept-and-document remaining flag TOCTOU like Fjall §B13 does; derive `size`
  updates only inside that helper. Add a same-key concurrent-writer regression test
  (`test_cached_concurrent_access` currently uses 50 *distinct* keys, so this family is
  untested).

### 2.2 — high — *(primary: 1.1)* — `MemBufferStore::get_many` missing the #539 tombstone-poisoning guard
- Same root defect as §1.1 (full write-up there, including this lens's additions: the window
  spans the whole backend round-trip, covers `Live` values too, and `get_many` has zero test
  coverage).

### 2.3 — medium — InMemoryStore::set: remove+re-insert update path built on a false premise ("no update-in-place API") — swallows a racing Duplicate so a later-completing `set` can return Ok while its value never lands
- File:line: `crates/shamir-storage/src/storage_in_memory.rs:115-135`; contradicting
  evidence: `src/storage_mirrored.rs:563-566` (lists `upsert_sync` among TreeIndex
  mutations) and vendored `scc-3.8.4/src/tree_index.rs:393` (`pub fn upsert_sync(&self, key:
  K, val: V)`); crate dep is `scc = "3.8"`. *(primary: also flagged by correctness-tdd §1.7,
  rated low there)*
- Issue: The comment claims "`scc::TreeIndex` has no update-in-place API so 2 traversals are
  unavoidable" — false for scc 3.8.x, which provides single-traversal `upsert_sync`. Beyond
  the cost claim being wrong, the Err branch (`remove_sync(&k); let _ = insert_sync(k, v)`)
  has a multi-writer interleaving where BOTH concurrent setters take the Err branch, A
  removes, B's remove no-ops, A inserts its value, and B's final `insert_sync` fails
  Duplicate — discarded silently — so **B completes `Ok(updated=true)` but its value never
  reaches the store** (A's, the earlier caller's, wins). This is distinct from (and worse
  than) the acknowledged "brief absence" window in :126-128. Exposure is bounded by the same
  engine-level serialization Fjall §B13 leans on ("the engine never issues two concurrent
  `set` calls for the same key"), but unlike §B13 that acknowledgment is absent here and the
  safer primitive is one call away. Note: `MirroredStore::set`'s error-atomicity argument
  relies on primary writes being infallible (which holds), but nothing there orders two
  concurrent setters either.
- Failure scenario: Any non-engine/tooling caller issuing two overlapping `set`s on one
  hybrid-table key; the second call acks success yet restart-time hydration (which streams
  the primary/mirror) resurrects only the first value.
- Suggested fix: Replace the Err branch with `self.data.upsert_sync(k, v)` (keep the
  `insert_sync` fast-path for the fresh-key case if desired); flag accuracy degrades to
  documented-best-effort (parity with Fjall/MemBuffer semantics), value-loss disappears.
  Correct the comment; add a two-task same-key set test asserting the completing-later write
  wins.

### 2.4 — medium — FjallStore::submit blocks the tokio executor thread when the 1024-slot worker queue fills
- File:line: `crates/shamir-storage/src/storage_fjall.rs:92-93` (`sync_channel(1024)` —
  verified), :188-201/:194-208 (`tx.send(...)` called directly in `async fn submit`), call
  sites :330-334 (`insert`) and :496-500 (`transact`). *(primary: also flagged by
  performance-hotpath §4.6 — rated low there — and error-handling-lifecycle §6.2)*
- Issue: Pillar 2 requires I/O-bound ops to be async / CPU-blocking work off the runtime.
  Here the enqueue onto the OS-thread worker is a synchronous
  `std::sync::mpsc::SyncSender::send` executed on the async caller's task. The comment says
  "a full queue simply parks the submitting task" (:90-91) — it actually **parks the runtime
  worker thread** running that task (blocking send, not task park): every other task
  scheduled on that core-thread stalls too. Under sustained commit pressure (batch commits
  are slow disk ops on the drain thread) ≥N_workers blocked submitters parked across
  different tokio workers removes the executor from service until the single fjall drain
  thread catches up — a latency cliff/live-lock shape rather than backpressure. FIFO
  ordering and backpressure intent are sound (contrast: the deliberate 1024 bound prevents
  OOM); the wait mechanism is what violates the model.
- Failure scenario: A 4096-task fan-in bulk import (`transact`/`insert` storm while the
  worker drains fsync-bound commits): 3072 tasks sit blocked on `send` occupying tokio
  worker threads; remaining runtime capacity starves — unrelated timer/IO tasks on those
  workers stop firing until the drain drains; SLOW/TIMEOUT-class symptoms under load.
- Suggested fix: Keep the single OS-thread worker + bounded channel, but gate submission
  with a `tokio::sync::Semaphore` acquired via `.await` before the blocking `send`
  (preserves the no-extra-hop property and bounds in-flight submitters to the queue depth);
  or `try_send` first and hop the rare blocking send into `spawn_blocking` (SyncSender is
  Clone/Send); or switch the front half to `tokio::sync::mpsc::channel(1024)` with
  `.send().await` consumed by the dedicated thread via a small adapter — preserves the
  measured worker wins, removes the sync-block-in-async case.

### 2.5 — low — moka cache built with default `RandomState` instead of workspace `THasher` (pillar 4)
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:232-255` (`build_cache` ends in
  plain `.build()`; the `THasher` import is already present at :84 for the dirty map).
- Issue: CLAUDE.md pillar 4 names `THasher` the default for every hash-keyed structure.
  `MemBufferStore` sits directly on the default disk stack (`BoxRepoFactory`:
  MemBuffer→Fjall), so every hot op hashes RecordKey twice+ (cache get + insert; weigher
  lookups) via SipHash-class hashing instead of FxHash. moka 0.12 supports this
  (`CacheBuilder::build_with_hasher`, verified in vendored moka 0.12.x builder docs); no API
  obstacle.
- Suggested fix: `builder.build_with_hasher(shamir_collections::THasher::default())`.

### 2.6 — low *(as tagged here)* — *(primary: 4.2)* — `InMemoryStore` iter_stream/scan_prefix_stream eagerly materialize the whole result set under one pinned epoch Guard
- The performance lens carries the full write-up (it rated the same defect **high** — the
  severity of record). This lens's added facet: the `scc::Guard` pinned across the whole
  collect delays EBR reclamation of nodes removed concurrently during large scans.

### 2.7 — low — CachedStore::reload is non-atomic clear→refill against live traffic
- File:line: `crates/shamir-storage/src/storage_cached.rs:306-327`. *(primary: also flagged
  by correctness-tdd §1.8 — the reload half of its size-counter-drift nit)*
- Issue: `clear()` + `size.store(0)` followed by a streamed refill: concurrent Async-mode
  `cache_upsert`s can repopulate keys mid-refill whose values then lose the insert race
  against older streamed rows (stale refill overwrite of a fresher write persists until the
  next same-key write/reload); `size` accounting drifts while readers/writers interleave
  with the refill counter bumps. Reads are safe (fall-through to inner). Likely DDL-time-only
  usage today, hence low.
- Suggested fix: Document reload as requiring quiescence (it is a resync/debug surface), or
  snapshot-diff into the live tree rather than clear-first.

### 2.8 — nit — *(primary: 1.6)* — Cross-path write ordering inside FjallStore rests only on fjall's internal journal mutex arrival order
- Folded into §1.6 facet (b); the standalone nit record is preserved there
  (`storage_fjall.rs:337-389` `set`, :550-580 `remove`, :17-47 worker rationale).

---

## 3. security-crypto

Boundary verdict (kept for calibration parity): no auth/crypto/TLS surface lives here and
the crate contains **zero `unsafe` blocks** (the only grep hit is a doc sentence in
`key_bytes.rs` explaining why an `unsafe` union layout was *rejected*). The one untrusted
input it owns — tampered on-disk mirror content at hydration — is handled well
(`MirroredStore::new` re-runs the allowlist classifier against every streamed entry and
skips+warns on drift — `storage_mirrored.rs:276-284`, backed by classifier-exhaustiveness
and hydration-drift tests). Timing side-channels: nothing here compares secret material, so
`KeyBytes`' non-constant-time slice equality is not exploitable as written.

### 3.1 — medium — Store names passed to the durable engine unvalidated
- File:line: `crates/shamir-storage/src/storage_fjall.rs:229-245` (`FjallRepo::store_get`),
  :247-262 (`store_delete`); `copy_store` (types.rs:488) composes onto this unchecked too.
- Issue: Both methods forward `name.as_ref()` straight into
  `Database::keyspace(&table_name, ...)` (and `delete_keyspace`) with no validation
  whatsoever: empty string, whitespace-only, control characters, absurd length,
  delimiter/path-flavored characters, or names engineered to collide with the engine's
  composed prefixes (`__data__<t>` / `__info__<t>` / `__history__<t>`) are all accepted and
  become durable on-disk artifacts. Whether fjall internally rejects pathological names was
  not verified; the crate neither relies on nor documents any guarantee, so the boundary
  simply trusts every caller. (Related, distinct: §5.7 create-on-read semantics.)
- Failure scenario: If a client-controlled DDL table name reaches `Repo` uncanonicalized
  (validation lives outside this crate), one request can mint or delete persistent storage
  artifacts under manipulated names, alias across composed store namespaces after a rename
  cycle, or wedge `stores_list()` consumers with invisible/control-character names.
- Suggested fix: Validate once at the `Repo` boundary — reject empty, over-length, and
  non-printable/non-ASCII names with `DbError::Validation`; canonicalize before calling
  fjall. Cheap O(1) guard on a cold path, converts a transitive trust assumption into a
  checked invariant.

### 3.2 — low — "Fresh random 128-bit id" claim behind skipping the insert collision probe is false (timestamp + 64-bit PRNG tail)
- File:line: `crates/shamir-storage/src/storage_fjall.rs:152-156` (`exec_insert`) and
  :324-329 (`Store::insert`); same claim in `benches/storage_fjall_pump.rs:96-101`.
- Issue: Both comments justify dropping the pre-insert `contains_key` probe with
  "`RecordId::new()` is a fresh random 128-bit id ... ~2^-128". The referenced
  implementation says otherwise (`shamir-types/src/types/record_id.rs:24-54`, :80-90): bytes
  `[0..8]` are wall-clock microseconds (fully predictable), bytes `[8..16]` come from a
  thread-local **Xoshiro256++** — deliberately *not* a CSPRNG, seeded once per thread from
  OS RNG. Predictability is 50% of the id and the random part carries 64 bits from an
  xoshiro stream that is computationally invertible/predictable after ~32 observed
  consecutive outputs per thread. The engineering *decision* (skip the probe) remains sound
  — distinct-microsecond timestamps dominate separation and the same-microsecond tail
  birthday bound is ample — but the written security argument overstates it by 2^64 and by
  PRNG strength, and other files repeat it.
- Failure scenario: Today nothing authenticates on id unguessability, so impact is latent.
  If any future feature starts treating record keys as opaque unguessable tokens (share
  links, presigned-style record URLs, lottery-on-key), this comment will have waved the
  design through under a "128-bit random" justification the implementation does not provide.
- Suggested fix: Correct the comments in place: "monotonic-ts-prefixed id with a 64-bit
  Xoshiro256++ tail; unique-by-construction for insert, **not a secret / not
  CSPRNG-backed**". One-line edits, keeps the perf decision intact while deleting the
  misleading premise.

### 3.3 — low — User-influenced keys enter non-keyed FxHash maps despite the documented "no untrusted hash inputs" premise
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:154` (field) and :298-299
  (construction): `dirty: DashMap<RecordKey, Slot, THasher>`; `storage_in_memory.rs:18,24`:
  `stores: TDashMap<String, _>` keyed by store name.
- Issue: CLAUDE.md pillar 4 trades away RandomState DoS protection because "we don't accept
  untrusted hash inputs here". Two structures in this crate break that premise in spirit:
  `dirty` is keyed by *caller-supplied* `RecordKey`s, and secondary-index posting keys embed
  indexed field values verbatim (per `storage_mirrored.rs:165-172`'s own key-shape
  description), i.e. attacker-chosen bytes on an ingestion path; `InMemoryRepo.stores` is
  keyed by table/store names that ultimately originate in DDL. FxHash is a multiply-xor
  construction whose collisions are trivially mass-manufactured, unlike SipHash.
- Failure scenario: A writer flooding crafted colliding posting keys while the write-back
  buffer sits undrained (default 500 ms tick) concentrates those entries into one dashmap
  shard; subsequent `set`/`get` probes and `snapshot_overlay_sorted`'s clone+sort of the
  overlay (`storage_membuffer.rs:576-595`) skew superlinearly on that shard — a bounded but
  measurable remote write-path latency amplifier. `InMemoryStore.data`'s `scc::TreeIndex`
  (ordered B+-tree) is unaffected; only the two hash maps above are exposed.
- Suggested fix: Either (a) document why the premise still holds (prove posting values are
  canonicalized/length-capped upstream such that collision farming is pointless), or (b)
  give just these two externally-influenced maps a keyed BuildHasher (SipHash/RandomState) —
  their access patterns are buffered/write-side, not the ultra-hot lock-free reads pillar 4
  optimizes for.

### 3.4 — low — Raw key bytes — including attacker-influenced indexed values — embedded in error messages
- File:line: `crates/shamir-storage/src/storage_fjall.rs:419`;
  `storage_in_memory.rs:111,140`; `storage_membuffer.rs:797,810`.
- Issue: `DbError::NotFound(format!("record not found: {:?}", key))` (and `KeyExists`)
  interpolate the full key via `Debug`. Posting keys carry indexed field values of *other*
  columns' data; these error strings flow up through engine/wire layers and log aggregation.
  Rust's `escape_debug` escapes `\n`/`\r`/controls (so single-line logs hold and
  forgery-via-newline is blocked), but it leaves printable Unicode intact — including BiDi
  overrides (U+202E etc.) and zero-width characters — so log/terminal spoofing and
  cross-record value leakage via error text are both possible.
- Failure scenario: A query touching a missing posting key surfaces fragments of some
  record's indexed value inside an error string shown to a different tenant/console; or
  renders convincingly-reversed log lines via injected BiDi characters inside a crafted
  indexed value.
- Suggested fix: For these specific call sites, render keys as bounded hex
  (`hex(&key[..min(key.len(), 16)])` style helper) instead of `{:?}` — one small local
  formatter, no API change.

### 3.5 — nit — `KeyBytes::Deserialize` allocates an unbounded blob before any size check
- File:line: `crates/shamir-storage/src/key_bytes.rs:308-313`.
- Issue: Deserialization goes through `serde_bytes::ByteBuf::deserialize`, materializing the
  entire input allocation before `from_slice` runs; there is no maximum-length guard. Today
  safe (callers are WAL/bincode/rmp-serde boundaries that own frame limits, and the type is
  unused by production per module docs — see §7.2 for why that doc claim itself is stale),
  but plan doc section 5.3 anticipates flipping `RecordKey` to `KeyBytes` across the WAL/
  client-wire paths — at that point a hostile frame chooses the pre-allocation size subject
  only to upstream framing.
- Suggested fix: When the alias flip lands, gate the constructor: deserialize, then reject
  `len > MAX_RECORD_KEY_BYTES` (tie to schema/tunable constants) returning a
  `de::Error::invalid_length`.

---

## 4. performance-hotpath

Theme verdict (kept for calibration parity): the disk-tier backends (fjall, membuffer,
cached) show clear evidence of the 2026-07-06 audit fixes — incremental cursor scans,
zero-copy reads, bounded write-worker channel — but the same fixes were never carried into
`InMemoryStore`, and test coverage (~93 tests) asserts nothing about laziness or memory
bounds on exactly the paths below, which is how these survived.

### 4.1 — high — Reverse range streams drain the ENTIRE range into RAM before reversing; not overridden by InMemory/Cached/Mirrored
- File:line: `crates/shamir-storage/src/types.rs:376-384` (trait default) + :391-412
  (`default_reverse`); missing overrides at `storage_in_memory.rs:103-258` (implements
  `iter_range_stream` at :171 but no reverse) and `storage_cached.rs:414-717` (no
  `iter_range_stream*` overrides at all); inherited through `storage_mirrored.rs:431-439`.
- Issue: The default `iter_range_stream_reverse` composes the forward range stream with
  `default_reverse`, which `extend`s every batch into one `Vec` before yielding ("Memory ~ N
  items" per its own doc). Consumers of reverse order are precisely the early-exit workloads
  — `lookup_last_k`, `lookup_max`, `ORDER BY ... DESC LIMIT K` (named in the method's own
  doc) — yet each pays a full-range drain plus an O(range) resident allocation even for K=1.
  Because `scc::TreeIndex::Range` implements `DoubleEndedIterator` (`next_back`, verified
  against scc 3.8 docs), InMemoryStore can drive it natively, and
  `storage_fjall.rs:430-483` already proves the incremental reverse-cursor pattern this
  crate prefers. (Fix-order note: §1.3 — the InMemory forward resume body violates the
  `Bound::Excluded` contract — should be fixed first so the pattern being mirrored is
  sound.)
- Failure scenario: A hybrid table (`MirroredStore`, primary = InMemoryStore) or cached
  table with millions of sorted-index postings receives `lookup_last_k(k=10)`/DESC page
  requests; each request clones and holds all matching entries before returning one batch.
  Memory spikes scale with total range size, latency is linear in N for constant-K reads,
  and concurrent such reads multiply the transient allocations.
- Suggested fix: Override `iter_range_stream_reverse` in `InMemoryStore` with a per-batch
  guarded `.range(..).rev()` walk that seeks to the last key in-bound and resumes downward
  past it (`Bound::Excluded(last)`); have `CachedStore` do the same over its TreeIndex cache
  (its `scan_prefix_stream`:587 already shows the repeated-bounded-requery shape). Keep the
  trait default as documented fallback only.

### 4.2 — high — InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole corpus before the first yield
- File:line: `crates/shamir-storage/src/storage_in_memory.rs:153-159` (`iter_stream`),
  :240-247 (`scan_prefix_stream`). *(primary: also flagged by concurrency-lockfree §2.6 —
  rated low there, adding that the pinned `scc::Guard` across the whole collect delays EBR
  reclamation of concurrently-removed nodes)*
- Issue: Both methods `collect()` ALL matching `(key, value)` pairs into a `Vec` while
  holding an epoch guard, then hand the vec to the stream, which just drains it in
  batch_size chunks. This is exactly the eager-collect anti-pattern audit
  `2026-07-06-perf-radical-o-notation` §1.3 removed from `CachedStore`
  (`storage_cached.rs:521-526` documents the fix) and from fjall/membuffer — but it was
  never applied to the in-memory backend. A consumer wanting only the first batch
  (LIMIT-style pulls, `copy_store`'s early error paths) still pays O(N)/O(matches) clones +
  a single large allocation up front; `TreeIndex::iter` under a fresh guard each round-trip
  would keep memory O(batch_size). Note the SAME FILE's `iter_range_stream` (:171-232)
  implements the correct short-lived-guard + resume-key incremental pattern — the
  inconsistency is within one impl block (and §1.3 shows even that body needs its resume
  discipline corrected).
- Failure scenario: `MirroredStore::new` hydration streams via `mirror.iter_stream` (fine),
  but any later `scan_prefix_stream` over the hybrid primary (e.g.
  `SortedIndexManager::rekey_postings` re-scans, index lookup warming) allocates the full
  match-set twice transiently (collect vec + per-batch drained vecs) regardless of consumer
  appetite; under concurrent scans these snapshots compound, and large scans pin EBR garbage.
- Suggested fix: Convert both methods to the :184-231 pattern: open a guard, collect up to
  `batch_size` items starting after a resume key (for prefix: lower bound = max(resume,
  prefix), stop when a key exits the prefix), drop guard, yield, repeat. Total work
  unchanged; peak memory drops to O(batch_size) and early-exit consumers pay only what they
  drain. Align the README claim or scope it to Fjall/Cached.

### 4.3 — medium — MemBufferStore::transact drains the ENTIRE dirty buffer before every transact
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:1037`
  (`self.drain_all().await?` inside `transact`; `drain_all` itself at :598-618 also
  snapshots with `batch_size = usize::MAX`, :600/:612).
- Issue: Only op-touched keys need flushing before delegating to `inner.transact` (a pending
  dirty value v1 for key k MUST land before the batch writes v2 directly to inner, else the
  next drain revives stale v1). Instead the code flushes every dirty entry in the buffer —
  unrelated point-writes included — to disk synchronously inside each transact. That is
  write amplification proportional to total unflushed traffic, not to |ops|: the same
  read/write-triggered-drain class audit §2.3 (task #530) removed for scans ("a full flush
  is no longer required just to read" — yet a transact forces one). Additionally `drain_all`
  calls `drain_once(usize::MAX)`, snapshotting the whole dirty DashMap (keys + cloned values)
  into RAM in one shot rather than in `flush_batch_size` chunks.
- Failure scenario: A table behind `CachedStore -> MemBufferStore -> fjall` mixes steady
  single-row `set`s (buffered, flushed on the 500 ms tick) with a moderate tx rate calling
  `transact`. Every transact flushes whatever happens to be pending — potentially hundreds
  of MiB of accumulated buffered rows (64 MiB default `max_bytes` worth of cache alongside)
  — turning one small batch commit into a full write-back of unrelated data, defeating the
  fsync batching the buffer exists to provide, and stalling the commit for the drain
  duration.
- Suggested fix: Pre-drain ONLY the keys appearing in `ops` (build the key set, snapshot
  those dirty entries via `dirty.get`, apply their values with `set_many`/`remove_many`
  targeting just those keys, then CAS-clean via the same `remove_if(slot == snapshot)`
  discipline used at :1060-1081). Chunk the flush-path snapshots in `drain_once(batch)`
  units so even legitimate full drains don't spike resident memory to O(all dirty).

### 4.4 — medium — CachedStore WriteMode::Async uses an UNBOUNDED write-behind channel
- File:line: `crates/shamir-storage/src/storage_cached.rs:242`
  (`mpsc::unbounded_channel::<CacheWriteJob>()` — verified); jobs carry owned values:
  `CacheWriteJob::Set { key, value }` at :55, enqueued at :450/:503.
- Issue: Async-mode `set`/`remove` enqueue onto a single worker through an unbounded channel
  with no high-watermark, cap, or admission signal. `pending_writes` counts the backlog but
  nothing acts on growth; each queued `Set` holds its full `Bytes` value in addition to the
  copy already upserted into the cache at :437. One serialized worker draining at `inner`'s
  write rate is the only relief. Contrast `storage_fjall.rs:85-93`, which deliberately chose
  `sync_channel(1024)` for this identical pattern with the explicit rationale "a pathological
  fan-out can't OOM the queue" — the lesson was applied at the disk worker but not at the
  cache wrapper sitting above it.
- Failure scenario: Data-tier store in Async mode against a backing store that slows (cold
  cache, compaction stall, network volume): sustained producer rate > single-worker drain
  rate grows the queue without limit; memory rises by two copies per pending op until OOM.
  No telemetry surfaces depth except polling `pending_writes`.
- Suggested fix: Bound the queue (e.g. `async_channel`-style bounded or
  `tokio::sync::mpsc` with capacity ~ the fjall worker's 1024) and make submitters await
  send (async-aware backpressure), or keep std channel but route the rare-full case through
  `try_send` + async wait. Optionally expose a high-watermark log/metric off the existing
  `pending_writes`.

### 4.5 — medium — Trait-default range filter scans PAST the upper bound forever
- File:line: `crates/shamir-storage/src/types.rs:419-447` (`default_range_filter` loop keeps
  consuming batches and filtering after keys exceed `end`).
- Issue: The input stream is contractually ascending (`Store::iter_stream` ordering
  guarantee, types.rs:293-302), so once a key exceeds `end_inclusive` every subsequent key
  does too — but the filter keeps draining the stream to the end, discarding everything. Any
  backend relying on this default pays O(pos(end)..N) wasted traversal instead of stopping at
  the boundary. Concrete victim in-crate: `CachedStore` has no `iter_range_stream` override,
  so upper-bounded range/order queries on the cached tier run the default filter over its
  incremental full-store cursor; `InMemoryStore` is unaffected (native override). No
  correctness issue; pure O(x→0) miss.
- Failure scenario: An upper-bounded `iter_range_stream(Some(start), Some(end))` on a
  CachedStore covering the top slice of a large store walks and clones-checks every key
  beyond `end` — cost proportional to what lies ABOVE the requested window's end, growing
  with corpus size for a fixed-size query.
- Suggested fix: Track a `done: bool`; inside the filter closure return-based exit isn't
  enough across element boundaries — set `done` when the first out-of-window key is seen
  (`k > end`), then break the outer batch loop instead of pulling further batches.

### 4.6 — low — *(primary: 2.4)* — FjallStore::submit blocks the async caller thread when the bounded queue fills
- Same root defect as §2.4 (the perf lens rated it low, noting bulk parallel loaders can
  plausibly cross 1024 outstanding `insert`s since every submitter parks until its own job
  completes).

### 4.7 — nit — Minor allocations/clones on batched paths
- Files: `crates/shamir-storage/src/storage_membuffer.rs:1245` (`miss_keys.clone()` — extra
  key-vector clone per `get_many` containing misses; keys are `RecordKey`, mostly inline, so
  cheap but avoidable by zipping `miss_idxs` against kept-key references);
  `storage_fjall.rs:617` and `:682` (`Vec::with_capacity(256)` hardcoded instead of
  `min(batch_size, 256)` — over-allocates for small batches, reallocates once for larger).
- Issue/fix: Cosmetic constant-factor cleanups on batch paths; no behavioral risk either
  way. Listed for completeness, not as debt demanding action.

Theme-relevant non-findings preserved from the perf lens (for the record): `FjallStore::
set`/`remove` double LSM lookups are a bench-adjudicated trade-off with sanctioned
flag-free fast paths (`storage_fjall.rs:394-403`/:585-593) — not a finding; MemBuffer's
dirty-map value duplication (:143-154) is documented, bounded-ish by the flusher, with its
flush-failure residual acknowledged in-code (:187-192) — §4.3 addresses the amplification
side, and the writer-side high-watermark question is adjacent, deliberately not raised.

---

## 5. api-wire-protocol

Surface verdict (kept for calibration parity): the `Store`/`Repo` trait documentation is
unusually strong — ordering guarantees stated as correctness contracts, honest capability
disclosure via `supports_atomic_transact` (F-77/F-85), and an exemplary `KeyBytes`
byte-identity suite against bincode and rmp-serde. The gaps are serialization/versioning and
per-backend contract fidelity.

### 5.1 — high — Persisted `MemBufferConfig` wire format has no versioning guardrails despite a "stable wire-format" claim
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:92-126`.
- Issue: The doc comment says "Stable wire-format (serialized into `info_store` by the DDL
  layer)", and the struct is a plain `#[derive(Serialize, Deserialize)]` over 5 fields
  (`max_bytes`, `max_entries`, `ttl_ms`, `flush_interval_ms`, `flush_batch_size`) — no
  `#[serde(default)]`, no version/tag field, no golden-bytes test pinning today's encoding.
  Per `Cargo.toml`, this blob lands in the engine's info_store via bincode at the DDL
  boundary (i.e., on disk inside `__info__<t>`, mirrored through to fjall).
- Failure scenario: Any future change — adding a field (bincode reads old blobs as short →
  error or misaligned garbage), reordering fields, widening `flush_interval_ms`/`ttl_ms`
  types — breaks deserialization of every previously-written database's buffer config at
  open/DDL-reload time. There is no migration hook to catch it, so an existing deployment
  either fails to open or silently loses its buffer config.
- Suggested fix: Add `#[serde(default = "...")]` per field (cheap insurance even under
  bincode's self-describing-hostile format) plus an explicit envelope/version field before
  any further schema churn; add a round-trip golden test that serializes today's default
  config bytes and asserts byte-equality forever, so an accidental format change fails CI
  instead of production opens.

### 5.2 — medium — `batch_size == 0` is unspecified: InMemoryStore yields empty batches forever; fjall/cached silently return zero results
- File:line: trait contract `types.rs:310` (`iter_stream`), :336 (`scan_prefix_stream`) — no
  stated precondition; broken divergent impls at `storage_in_memory.rs:161-169` and
  :249-257; silent-empty behavior in `storage_fjall.rs:596-646`/`:656-723` and
  `storage_cached.rs:540-584`/`:600-645`.
- Issue: Nothing documents that `batch_size` must be > 0. `InMemoryStore`'s stream loops do
  `take = min(batch_size, entries.len()); drain(..take); yield` — with `batch_size == 0` and
  a non-empty corpus this yields `Ok(vec![])` infinitely and never terminates. Fjall and
  CachedStore use `.take(batch_size)` (= 0 items → empty batch → break), so they end
  immediately having yielded nothing at all even though data exists. Only
  `merge_overlay_stream` defends itself (`storage_membuffer.rs:661`, `batch_size.max(1)`),
  which shows the hazard is known but handled inconsistently.
- Failure scenario: A caller derives batch size from a tunable/config that computes to 0:
  full-table scans on disk backends report "no rows" (silent wrong answer feeding index
  scans/posting lists), while the identical call on the in-memory backend never completes (a
  hang — classified as a bug by repo policy, but by the *caller*, who has no way to know 0
  is invalid).
- Suggested fix: Either clamp defensively (`batch_size.max(1)` everywhere, matching
  `merge_overlay_stream`) or add `debug_assert!(batch_size > 0)` plus one sentence in the
  trait docs stating the precondition and the exhaustion semantics ("a final batch may be
  shorter; empty batches are never yielded").

### 5.3 — medium — `set`/`remove` created/existed flag precision varies by backend and write mode, with no capability disclosure
- File:line: contract `types.rs:36-39` (`set`: "Returns true if created"), :66-67 (`remove`:
  "true if existed"); divergences in `storage_membuffer.rs:763-788` (best-effort,
  cache/dirty-only, "false (presumed new)" after eviction — acknowledged inline at :764-768
  but not in trait docs), `storage_cached.rs:435-464` and :493-513 (Async-mode flags derived
  from cache state only). Related concrete bug: §1.4 (remove's flag is *wrong* for inner-only
  keys — a distinct defect from this disclosure gap).
- Issue: The trait documents one semantic for `bool`; implementations actually deliver three
  tiers: strict-but-TOCTOU (Fjall, documented inline at `storage_fjall.rs:358-364`),
  best-effort-local (MemBuffer — deliberately consults only dirty+cache, never inner, so an
  evicted-key update reports `created = true`), and mode-dependent (CachedStore Sync vs
  Async). This crate already established the correct pattern for exactly this problem —
  `supports_atomic_transact` (`types.rs:285-287`) was introduced because another
  undocumented capability gap caused MirroredStore to violate an overpromised contract
  (F-77) — but flag precision was left as free-text commentary scattered in impl comments.
- Failure scenario: A caller using `set`'s return to decide insert-vs-update bookkeeping
  (e.g., bumping a counter once per genuinely-new key) gets wrong counts when running over
  MemBuffer-wrapped backends whenever moka evicted the key, with no compile-time or runtime
  signal that the guarantee differs from the Fjall build it was developed against.
- Suggested fix: Document the tier each backend delivers in the trait method doc (or mirror
  the Fjall precedent: note where the strictness boundary is); longer-term, extend the
  F-77-style pattern with e.g. `fn strict_exists_flags(&self) -> bool` if callers ever need
  to gate on it.

### 5.4 — medium — Cross-crate wire-format literal duplicated privately: `[0,0,0,0]` system-record prefix re-encoded in storage_mirrored
- File:line: `crates/shamir-storage/src/storage_mirrored.rs:41-48`
  (`SYSTEM_RECORD_PREFIX: [u8; 4] = [0,0,0,0]`, kept local because the canonical constant is
  private) vs `crates/shamir-types/src/types/record_id.rs:18` (private
  `const SYSTEM_RECORD_PREFIX: &[u8] = &[0, 0, 0, 0];`); consumed by the durability
  classifier at `storage_mirrored.rs:173-198`.
- Issue: A 4-byte wire constant that decides which keys survive a hybrid-table restart
  exists in two crates with no shared definition. The local copy is honestly annotated, and
  the exhaustiveness test (`storage_mirrored_tests.rs:244`, building keys via the real
  `RecordId::system`) would catch an encoding change indirectly — but the guard is a
  behavioral proxy, not the type-level guarantee the module's own care level implies.
- Failure scenario: The record-key migration plan this crate is mid-flight on
  (`docs/dev-artifacts/design/record-key-128-migration-plan.md`) touches exactly these
  encodings; if `RecordId::system`'s prefix/padding changes without the allowlist match set
  following, classifier hits go to zero and every durable-config key silently becomes
  ephemeral — table/index/buffer config stops surviving restart, logged only as hydration
  drift warnings (if any entries existed to warn about).
- Suggested fix: Export `pub const SYSTEM_RECORD_PREFIX: [u8; 4]` (or a
  `RecordId::system_prefix()` accessor) from shamir-types and use it here; keep the local
  copy only if the borrow direction (types must not depend on storage) forbids it — in which
  case add a cross-crate test asserting the two literals stay equal.

### 5.5 — low — Public-API rustdoc drift: prefetch promise and phantom engines
- File:line: `types.rs:289-292` ("Uses concurrent prefetching: while yielding current batch,
  fetches next batch in background" — no implementation prefetches; every backend runs
  sequential cursor-resumed batches, and the crate README itself describes the design as
  lazy fetch-on-demand). Phantom backend references presented as live implementors/callers:
  `types.rs:96` ("sled, fjall, cached MUST override"), :134-141, :185-188, :343-347, :374
  (sled/redb/persy/nebari/canopy), `storage_in_memory.rs:80`, `storage_membuffer.rs:119-121`,
  `src/tests/types_tests.rs:149-150` — the crate README (`src/README.md:152-159`) explicitly
  documents these engines as removed/nonexistent. (The finding's other halves — the stale
  `key_bytes.rs` header and the drifted line-ref at `storage_fjall.rs:655` — are the same
  defects as §7.2 and §7.7 and are counted once there.)
- Issue: For a new implementor of `Store` (the primary audience of trait rustdoc), the
  contract text describes behavior that does not exist (concurrent prefetch) and an engine
  ecosystem that does not exist.
- Failure scenario: Documentation-only: someone implementing a new backend budgets effort
  for background-prefetch machinery or compares against phantom engines.
- Suggested fix: One surgical rustdoc pass: delete the prefetch sentence (or reword to
  "batches are fetched lazily, one cursor-resumed range per batch"); replace phantom-engine
  name-drops with "buffering backends".

### 5.6 — low — *(primary: 1.5)* — Shared backend-conformance suite skipped by CachedStore and MirroredStore
- Folded into §1.5 (the api lens's framing — "two of five `Store` implementations never run
  the crate's own agnostic conformance checklist; a delegation-path regression breaks the
  documented contract on those backends only" — is preserved there).

### 5.7 — low — `Repo::store_get` create-on-read semantics make typos durably materialize
- File:line: contract `types.rs:465-468` ("Retrieves a store by name. Creates it if it
  doesn't exist"); disk-side effect in `storage_fjall.rs:229-245` (fjall keyspace created
  via `KeyspaceCreateOptions::default` on every miss). Related, distinct: §3.1 name
  validation.
- Issue: There is no open-without-create probe on the `Repo` trait, so a read-path caller
  can never validate existence without mutating durable layout.
- Failure scenario: A misspelled table name anywhere on a read path materializes a real,
  empty fjall keyspace in the repository directory — visible in `stores_list()`, occupying
  metadata/journal space until manually cleaned, and indistinguishable from an intentional
  empty table downstream.
- Suggested fix: Add a default-implemented non-mutating check (e.g. `store_exists(name) ->
  bool`, cheaply answerable by all current backends) and route pure-validation callers
  through it; optionally log when `store_get` creates a new keyspace so accidental creation
  surfaces in logs.

### 5.8 — nit — Interface polish bundle
- Remaining items (the RecordStream and Tests-banner bullets are deduped into §7.4/§7.3):
  - Mixed key vocabulary on scan APIs: keys are `RecordKey`/`KeyBytes` but prefix/range
    bounds take raw `Bytes` (`types.rs:336, 354-358`), forcing boundary conversions in every
    backend (`RecordKey::from(prefix.clone())`). Consider accepting `impl AsRef<[u8]>`
    uniformly.
  - `Repo::copy_store` takes `&str` (`types.rs:488`) while sibling methods take generic
    `AsRef<str> + Send` — signature inconsistency in the same trait.
  - Double-prefixed message: `DbError::KeyExists(format!("Key already exists: {:?}", key))`
    renders as "Key already exists: Key already exists: ..." since the variant Display adds
    the same prefix (`storage_in_memory.rs:111` vs `error.rs:13-14`).
  - `error.rs` accumulates engine-domain variants (`Function`, `ValidatorRejected`,
    `ValidatorInvalid`, `IndexDrainInProgress`) in the lowest-layer crate; deliberate per
    its doc ("generic error"), but it couples every backend consumer to the engine's error
    taxonomy. Its `code()` wire mapping covers only 3 variants — fine, just ensure consumers
    know the mapping lives here.

Test-coverage notes preserved from the api lens: layout conforms to CLAUDE.md (per-module
`tests/` dirs with manifest-only `mod.rs`; test-only seams `#[cfg(test)]`-gated);
`key_bytes/tests/serde_byte_identity_tests.rs` is exemplary for a wire-format suite
(bincode + rmp-serde byte-identity against a local mirror of the WAL encoder, spanning
INLINE_CAP boundaries and cross-decode in both directions); the gap is only §5.6/§1.5.

---

## 6. error-handling-lifecycle

Discipline verdict (kept for calibration parity): error plumbing is broadly strong and
battle-tested — every fallible op returns `DbResult`, backend errors map into `DbError`
variants, production panics are confined to commented invariant violations (all verified as
genuinely unreachable: `storage_in_memory.rs:130,227`; `storage_fjall.rs:98,112,123,126,145`;
`storage_membuffer.rs:688,704`; `storage_cached.rs:160,409,422,449,502`), and
`drain_once` retains dirty entries on error, the §2.3 `remove_if` guard is regression-covered,
`MirroredStore` mirror-first ordering delivers honest error atomicity (thoroughly tested with
injected failures), and `CachedStore::flush` surfaces background failures exactly once
(#1082). The weaknesses are concentrated in resource lifecycle and untested error branches.

### 6.1 — high — `CachedStore::flush()` can hang forever if the async write-worker task dies before draining
- File:line: `crates/shamir-storage/src/storage_cached.rs:68-113` (worker loop), :243-249
  (`tokio::spawn`, handle discarded), :383-399 (`wait_for_async_writes`).
- Issue: In `WriteMode::Async`, `CachedStore` increments `pending_writes` per enqueued job
  and relies exclusively on the worker task to (a) `fetch_sub` after each job completes and
  (b) call `notify.notify_waiters()`. The task is launched with a discarded `JoinHandle`; if
  any `inner.set/remove` (an `Arc<dyn Store>` this crate does not control — it can be any
  wrapper, a foreign impl in tests/tooling, or scc/moka hitting an allocation panic) panics
  inside the worker task, or the runtime drops the task at shutdown mid-queue, the decrement
  + notify for every queued job never happens. `wait_for_async_writes` then loops:
  `pending_writes != 0` forever, and since `notify_waiters()` will never fire again, every
  subsequent `flush()` parks indefinitely. A durability-path deadlock of unbounded length;
  under house rules ("hangs are bugs") this is a defect even though the trigger requires an
  inner panic/cancellation. (The `Notify` before-check pattern itself follows tokio's
  documented race-free shape — given the worker stays alive.)
- Failure scenario: one failing/panicking inner write during a bulk load → all later
  `flush()` calls (the graceful-shutdown flush included) stall permanently instead of
  returning `Err`.
- Suggested fix: make the decrement+notify panic-safe — wrap each job iteration so
  decrement/notification run on unwind (a Drop guard over `pending_writes`), and/or await
  the worker's `JoinHandle` alongside (abort-on-death → surface
  `Err(DbError::Internal("async write worker died"))` from `flush()`), optionally add a
  bounded recheck with a plain atomic loop as a backstop.

### 6.2 — medium — *(primary: 2.4)* — Blocking `SyncSender::send` executed directly on tokio executor threads
- Folded into §2.4 (the error lens's framing — pillar-2 violation with SLOW/TIMEOUT-class
  symptoms under commit storms, and the suggested `spawn_blocking`/tokio-mpsc routes — is
  preserved there).

### 6.3 — medium — `MemBufferStore::Drop` silently discards a non-empty dirty buffer — zero observability
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:621-626` (`Drop`), dirty-buffer
  contract :49-52 (module doc).
- Issue: Dropping the store sets `shutdown` and wakes the flusher, which exits *before*
  draining; whatever is still in `dirty` (values not yet applied to `inner`) is dropped
  without any log, count check, or accessor. The crate itself established in audit §2.2
  (:348-360) that buffered writes dying silently is unacceptable ("dirty grows unboundedly
  with zero signal") and added a counter + log for the flusher case — but the drop path loses
  the same data class with *less* signal than the bug §2.2 fixed. A `Drop` cannot `.await`,
  but observing the loss costs nothing.
- Failure scenario: a consumer recreates/replaces a MemBuffer-wrapped store outside
  `apply_config`'s drain-first path (the only documented safe path); all ACKed-but-unflushed
  writes vanish while `inner` keeps stale values — undiagnosable afterwards.
- Suggested fix: in `Drop`, when `dirty_count > 0`, emit a `log::warn!` naming the store and
  entry count (and/or expose `dirty_count()` for callers/tests to assert orderly shutdown);
  document explicitly that drop-with-dirty = data loss by contract.

### 6.4 — medium — Missing error-path tests; audit-§2.2 telemetry is written but never read
- File:line: `storage_membuffer.rs:192,355` (`flush_errors` — no reader anywhere, not even a
  `#[cfg(test)]` accessor); `storage_cached.rs:446-462,499-510` (worker-channel-closed
  fallbacks); `storage_fjall.rs:199-207` (both `DbError::Internal` mappings in `submit`);
  `types.rs:488-503` (`Repo::copy_store` default partial-failure).
- Issue: No test constructs any of these states:
  - MemBufferStore background-drain failure: the §2.2 behavior (counter increment, error
    log, dirty retained + retried next tick) is completely uncovered, and `flush_errors` has
    no accessor, so the counter cannot ever be observed — dead telemetry, unverifiable
    claim.
  - CachedStore `set`/`remove` send-failure branch ("worker gone, write dropped":
    pending-count undo + loud log).
  - FjallStore `submit` error shapes (`Internal("write worker channel closed"/"dropped
    reply")`) mapped to match the old `spawn_blocking` semantics.
  - `Repo::copy_store`: nothing tests what state remains when `src.iter_stream` or
    `dst.set_many` fails mid-copy (relevant to RENAME TABLE — see §6.6).
- Failure scenario: regressions in exactly these branches (e.g. removing the pending-count
  undo, changing the retry discipline, breaking retained-dirty-on-error) land silently green.
- Suggested fix: failing-inner wrappers (already idiomatic in this suite: `FailingStore`,
  `FailingTransactMirror`) cover the first three cheaply; add a `#[cfg(test)]
  dirty_error_count()` accessor (and a failing-backend test asserting `flush_errors` bumps
  and dirty survives a failed drain).

### 6.5 — low — Cache eviction/deletion committed before the fallible backing op is acknowledged
- File:line: `storage_cached.rs:487-515` (`remove` evicts cache before `inner.remove`
  resolves — both modes), :427-467 (`set` Async branch populates cache before enqueue result
  known).
- Issue: On `Err` from the backing store the cache mutation is already durably applied
  locally. Sync mode self-heals (next `get()` read-through re-caches what `inner` still
  holds), but Async remove is worse: after the one-shot flush error (#1082 semantics), the
  key cache-misses into `inner`, which still holds the old value — the deleted key silently
  resurrects on later reads with no further signal, and reload/hydration makes it permanent.
- Failure scenario: backing store outage during Async-mode deletes → caller sees one `Err`
  from `flush()`, then reads resurrect every tombstoned key with no diagnostic.
- Suggested fix: hold a sticky negative marker (or re-tombstone on read-through hit of a
  failed-remove key) until the removal is confirmed, or at minimum log on the resurrection
  path; document the divergence window in the module doc.

### 6.6 — low — `Repo::copy_store` default impl leaves a partially-populated destination on failure
- File:line: `types.rs:488-503`. Related, distinct: §1.9 self-copy guard on the same method.
- Issue: Copy-then-orphan rename streams batches into `dst.set_many` with no compensating
  cleanup: a mid-stream error returns `Err` leaving a half-copied destination store that
  persists on disk and appears in `stores_list` forever. Retry convergence relies on
  overwrite-by-key idempotency, which breaks if source rows were removed between attempts
  (stale extras survive in dst). None of this is documented on the method.
- Failure scenario: RENAME TABLE fails partway → phantom `__data__<t>`-shaped store
  accumulates; a successful later copy over different src content merges stale keys.
- Suggested fix: either best-effort `store_delete(to)` on the error path (documented, orphan
  disposition matches DROP TABLE) or spell out the convergence/idempotency contract callers
  must honor.

### 6.7 — low — *(primary: 1.6)* — `FjallRepo::store_get` returns a fresh `FjallStore` per call — fragile per-instance worker lifecycle
- Folded into §1.6 facet (c) (spawn+join churn per transaction, handles outliving
  `store_delete`, the name-keyed `Arc` cache fix).

### 6.8 — nit — Error-source chains flattened; thread-spawn failure panics instead of `DbResult`
- File:line: `error.rs:92-96` (`From<CodecError>` → `err.to_string()`); most variants carry
  `String` rather than a typed source; `storage_fjall.rs:95-98`
  (`.expect("spawn fjall write worker thread")`).
- Issue: CLAUDE.md asks for `thiserror` with `#[from]` where natural; `std::io::Error` gets
  it, but `CodecError` (and fjall/DB errors generally) degrade to display strings, losing
  `source()` chains for diagnostics. Thread-spawn failure in `WriteWorker::spawn` panics the
  calling async context via `OnceLock::get_or_init` (can't propagate an error through it);
  defensible as near-fatal, but inconsistent with the crate's otherwise strict no-panic
  surface.
- Suggested fix: consider `#[source]`/typed variants where ergonomic (esp. `Codec`); make
  `WriteWorker::spawn` return `Option<WriteWorker>` handled as `DbError::Storage`/`Internal`
  if a hard dependency on graceful degradation matters.

---

## 7. style-claude-md

Structure verdict (kept for calibration parity): `src/tests/mod.rs` and
`src/key_bytes/tests/mod.rs` are re-export manifests only; no inline `#[cfg(test)] mod
tests` in implementation files (both test trees wired via the prescribed `#[cfg(test)] mod
tests;` pointers at `key_bytes.rs:315-316` and `lib.rs:32-33`; `membuffer_clear_race_hook`
keeps its logic in a cfg-gated sibling file); `lib.rs` is declarations + docs only;
one-file-one-primary-export holds (Repo+Store pairings are closely-coupled groups; private
helper enums serve exactly their owning store); `thiserror` used for `DbError`. The two
real clusters are function-local imports and stale residual comments.

### 7.1 — medium — Function-local imports violate the mandatory "Imports at the top" rule
- File:line: types.rs:395, :424, :489, storage_cached.rs:218, :307, storage_fjall.rs:451,
  :610, :673; also tests: storage_membuffer_tests.rs:426, storage_cached_tests.rs:321,
  storage_in_memory_tests.rs:264, storage_mirrored_tests.rs:1289,
  key_bytes/tests/hash_consistency_tests.rs:52. *(primary: also flagged by correctness-tdd
  §1.10's second half)*
- Issue: CLAUDE.md ("Imports at the top") requires all `use` statements in the file (or
  enclosing module) header, allowing only three documented exceptions (`use super::*`
  inside cfg(test) test modules; collision-justified single-method trait imports;
  cfg-gated/macro bodies). None of these apply here:
  - `use futures::StreamExt;` inside fn bodies/closures: types.rs (in `default_reverse`,
    `default_range_filter`, `Repo::copy_store`), storage_cached.rs (in `new_with_mode`,
    `reload`). No name is in collision scope, and sibling files already hoist exactly this
    import at top level (types_tests.rs:7, storage_cached_tests.rs:11,
    storage_mirrored.rs:36), so even the repo's own precedent contradicts the local
    placement.
  - `use std::ops::Bound;` inside the three `spawn_blocking` closures of storage_fjall.rs
    (`iter_range_stream_reverse`, `iter_stream`, `scan_prefix_stream`) — not cfg-gated,
    hoistable.
  - Test files: `use tokio::task::JoinSet;` mid-`#[tokio::test]` (x2),
    `use crate::storage_cached::CachedStore;` mid-test at storage_mirrored_tests.rs:1289
    (top-level imports there already include other storage modules, so no collision),
    `use futures::StreamExt;` inside the `collect_stream` helper,
    `use std::hash::DefaultHasher;` inside a test body where the sibling header import line
    (`std::hash::{BuildHasher, Hash, Hasher}`) could simply be extended.
- Failure scenario: none behavioral; it defeats the rule's purpose (single-glance dependency
  inventory per file, diff hygiene), and because enforcement is manual, each new stream/range
  method tends to copy the nearest local `use` rather than the header.
- Suggested fix: hoist all thirteen imports to their file headers (module headers for the
  nested test mods). Mechanical, zero-risk diff.

### 7.2 — medium — Stale module doc in key_bytes.rs claims the type is unused and that `RecordKey = Bytes`
- File:line: `crates/shamir-storage/src/key_bytes.rs:4-9` (module doc — verified during
  synthesis against `types.rs:9`). *(primary: also flagged by api-wire-protocol §5.5,
  rated low there as part of its rustdoc-drift bundle)*
- Issue: The doc says step 1 landed "with zero call-site changes anywhere else", that
  `types.rs`'s "`pub type RecordKey = Bytes;` alias is left untouched", and that KeyBytes is
  "currently unused by production code". Since then the alias flip happened: `types.rs:9`
  reads `pub type RecordKey = KeyBytes;`, making `KeyBytes` *the* production record key
  across every backend.
- Failure scenario: A maintainer reading only the module doc would believe `RecordKey` is
  still `bytes::Bytes` and that inline-vs-heap behavior is dormant/unexercised in production
  — e.g. reasoning wrongly about allocation costs, or "deferring" work that has actually
  shipped, or trusting serialization properties of `Bytes` that no longer apply on these
  paths (the serde byte-identity suite now guards every WAL/storage key).
- Suggested fix: update the doc's framing to describe state-after-step-2 (alias flipped;
  representation-invariance guarantees now load-bearing everywhere `RecordKey` flows),
  keeping the history references.

### 7.3 — low — Orphaned "// ===== Tests =====" banner comments left behind after tests moved out
- File:line: storage_in_memory.rs:260-262, storage_cached.rs:719-721,
  storage_fjall.rs:726-728. *(primary: also flagged by api-wire-protocol §5.8's bundle)*
- Issue: All three files end with the empty banner block marking where inline tests used to
  live. The tests now follow the documented layout in `src/tests/*.rs`; these residues imply
  inline tests should follow and invite re-adding them there (the exact anti-pattern
  CLAUDE.md's test-organisation section bans).
- Failure scenario: a contributor appends a new test under the banner, recreating an inline
  test block in an impl file.
- Suggested fix: delete the three dangling banners.

### 7.4 — low — Duplicate private `RecordStream` alias re-declared instead of importing the canonical one
- File:line: storage_membuffer.rs:628 (vs. canonical `pub(crate) use` target at
  types.rs:11-12); same duplication in tests/types_tests.rs:12-13. *(primary: also flagged
  by api-wire-protocol §5.8's bundle, which adds the API-surface argument)*
- Issue: `type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>,
  DbError>> + Send>>;` is re-declared locally although `crate::types::RecordStream` is
  `pub(crate)` and already imported cross-module by storage_mirrored.rs:33. The api lens
  adds the deeper point: `RecordStream` is `pub(crate)` yet is the return type of required
  **public** trait methods, so external implementors/consumers cannot name it at all — which
  is *why* the alias had to be duplicated twice. Two copies can drift independently (e.g. if
  the item type ever grows a third field or the error type narrows).
- Failure scenario: a future signature change to the canonical alias silently leaves
  MemBuffer's copy inconsistent, surfacing as a compile break or — worse if both happen to
  still unify — unnoticed divergence.
- Suggested fix: `use crate::types::RecordStream;` in storage_membuffer.rs (and in
  tests/types_tests.rs); delete the local aliases — and make the canonical alias `pub` so
  external `Store` implementors can name their return type.

### 7.5 — low — *(primary: 1.5)* — Shared batch/conformance suite never runs against CachedStore or MirroredStore
- Folded into §1.5 (the style lens's framing — "the two wrappers whose correctness depends
  on faithfully preserving/inheriting default batch semantics through delegation" — is
  preserved there).

### 7.6 — nit — storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files
- File:line: tests/storage_membuffer_tests.rs:728 (`mod audit_2_3`), :867
  (`mod clear_race_535`), :974 (`mod batch_insert_republish_535`).
- Issue: The test-organisation section prescribes splitting by topic into one file per
  related group within `tests/` (the pattern `key_bytes/tests/` follows properly). Here
  three self-contained fixture groups (~380 lines including mock `Store` impls) sit as
  inline submodules of one 1075-line file. Their imports are correctly placed at each
  submodule's header (the documented exception pattern), so this is purely about file
  granularity.
- Failure scenario: continued accretion; the next audit fixture gets a fourth inline mod
  instead of a file.
- Suggested fix: promote each `mod` to its own `audit_2_3_tests.rs` / `clear_race_tests.rs` /
  `batch_pause_tests.rs` under `src/tests/`, registered in `tests/mod.rs`.

### 7.7 — nit — Drifted line-number reference in a comment
- File:line: storage_fjall.rs:655. *(primary: also flagged by api-wire-protocol §5.5)*
- Issue: `scan_prefix_stream`'s doc says the resume pattern matches "iter_stream above (lines
  ~323)" — `iter_stream` now sits around line 596; hard-coded line numbers rot on every edit
  above them.
- Failure scenario: reader chases a stale pointer, wastes time, distrusts nearby comments.
- Suggested fix: drop the parenthetical line reference; name the method only.

---

## Finding counts

Raw lens-tagged findings (as filed across the 7 reports, matching the workspace SUMMARY.md
row: 0 crit / 7 high / 17 med / 19 low / 10 nit = 53):

| Lens | crit | high | med | low | nit | total |
|---|---|---|---|---|---|---|
| correctness-tdd | 0 | 1 | 3 | 3 | 3 | 10 |
| concurrency-lockfree | 0 | 2 | 2 | 3 | 1 | 8 |
| security-crypto | 0 | 0 | 1 | 3 | 1 | 5 |
| performance-hotpath | 0 | 2 | 3 | 1 | 1 | 7 |
| api-wire-protocol | 0 | 1 | 3 | 3 | 1 | 8 |
| error-handling-lifecycle | 0 | 1 | 3 | 3 | 1 | 8 |
| style-claude-md | 0 | 0 | 2 | 3 | 2 | 7 |
| **total (lens-tagged)** | **0** | **7** | **17** | **19** | **10** | **53** |

Deduplicated distinct-defect census (each root-cause defect counted once; deduped severity =
highest severity any lens assigned; nit bundles counted as 1 per the workspace counting
note; partial overlaps inside §1.8/§1.10/§5.5/§5.8 leave their non-shared halves as their
own defects):

| Severity | Lens-tagged findings | Distinct defects | Dedup groups / findings counted once |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 7 | 6 | 1.1 + 2.2 (get_many guard — one defect, two lenses); 2.1, 4.1, 4.2 (eager materialize — dedup of 2.6), 5.1, 6.1 |
| medium | 17 | 16 | 2.3 (dedup of 1.7) · 2.4 (dedup of 4.6 + 6.2 — one defect, three lenses) · 7.1 (dedup of 1.10's import half) · 7.2 (dedup of 5.5's key_bytes half); singles: 1.2, 1.3, 1.4, 3.1, 4.3, 4.4, 4.5, 5.2, 5.3, 5.4, 6.3, 6.4 |
| low | 19 | 13 | 1.5 (dedup of 5.6 + 7.5 — one gap, three lenses) · 1.6 (dedup of 2.8 + 6.7 — one structural fact, three lenses) · 2.7 (dedup of 1.8's reload half) · 7.3 (dedup of 5.8 bullet) · 7.4 (dedup of 5.8 bullet); singles: 2.5, 3.2, 3.3, 3.4, 5.5 (remainder), 5.7, 6.5, 6.6 |
| nit | 10 | 9 | 7.7 (dedup of 5.5's line-ref half); singles: 1.8 (remainder), 1.9, 1.10 (remainder), 3.5, 4.7, 5.8 (remainder), 6.8, 7.6 |
| **total** | **53** | **44** | 53 lens-tagged findings → 44 distinct defects |

---

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Port the #539 dirty-recheck guard into `get_many`'s fill loop** (re-probe
   `dirty_count`/`dirty.get(&k)` before each `cache.insert`, mirroring `get()` at
   `storage_membuffer.rs:840-880`) and add the hook-based `get_many` regression test.
   Closes **1.1 / 2.2** — the permanent read-your-write break under ordinary concurrent
   vectored reads.
2. **Give `CachedStore` one ordered cache-mutation helper** (single ordered worker for both
   modes, or collapse remove+insert into `upsert_sync` with fill/populate routed through the
   helper; derive `size` only inside it) + a same-key concurrent-writer test. Closes
   **2.1** — silent acked-write loss; the workspace's top-ranked storage risk.
3. **Make the Async write-worker panic-safe** (Drop guard over `pending_writes`, awaited
   `JoinHandle` surfacing `Err` from `flush()`, bounded recheck backstop). Closes **6.1** —
   durability-path flush hang.
4. **Version the persisted `MemBufferConfig`**: per-field `#[serde(default)]`, explicit
   version/envelope field, golden round-trip test pinning today's bytes. Closes **5.1**
   before any further schema churn bricks existing database opens.

**P1 — soon**

5. **Fix the scan-laziness class**: implement incremental reverse-cursor overrides for
   InMemoryStore/CachedStore (4.1); convert `iter_stream`/`scan_prefix_stream` to the
   per-batch guarded resume pattern (4.2, closing 2.6's guard-pinning facet with the same
   edit). Fix **1.3** (inclusive-resume → `Bound::Excluded`) first so the template body is
   sound.
6. **Stop `transact` from force-draining the whole dirty buffer** — pre-drain only op keys
   with the `remove_if(slot == snapshot)` CAS discipline; chunk `drain_once` snapshots.
   Closes **4.3**.
7. **Bound the CachedStore Async write-behind queue** with async-aware backpressure and a
   high-watermark signal. Closes **4.4**.
8. **Make `FjallStore::submit` async-safe** (semaphore-gated or `try_send`+`spawn_blocking`
   or tokio mpsc front half). Closes **2.4 / 4.6 / 6.2** (one defect, three lenses).
9. **Republish-guard family**: guard `MemBufferStore::transact`'s post-commit cache republish
   (1.2) via the same helper as item 1; replace `InMemoryStore::set`'s Err branch with
   `upsert_sync` and correct its comment (2.3, closing 1.7).
10. **Contract fidelity**: clamp/document `batch_size == 0` (5.2); fix or explicitly amend
    `remove`'s existed-flag contract for inner-only keys (1.4) and document the per-backend
    flag tiers (5.3) — same doc/test sweep.
11. **Lifecycle observability**: warn on drop-with-dirty + `dirty_count()` accessor (6.3);
    add the missing error-path tests and a readable `flush_errors` (6.4).
12. **Run the shared batch suite over CachedStore (both modes) and MirroredStore** (1.5,
    closing 5.6 / 7.5).

**P2 — backlog**

13. `THasher` for the moka cache (`build_with_hasher`). Closes **2.5**.
14. Validate store names at the `Repo` boundary (3.1); add `store_exists` non-mutating probe
    + creation logging (5.7).
15. Export `SYSTEM_RECORD_PREFIX` from shamir-types (or add the cross-crate literal-equality
    test). Closes **5.4**.
16. Security hygiene: correct the "random 128-bit id" comments (3.2); document or re-key the
    two untrusted-input FxHash maps (3.3); bounded-hex key rendering in error strings (3.4);
    `MAX_RECORD_KEY_BYTES` gate when the KeyBytes wire flip lands (3.5).
17. copy_store hardening: `from == to` guard (1.9) + best-effort destination cleanup or a
    documented convergence contract (6.6).
18. FjallStore worker model: scope the ordering invariant to a single handle, name the
    worker-vs-`spawn_blocking` interleave, share the worker per keyspace. Closes
    **1.6 / 2.8 / 6.7** (one structural fact, three lenses).
19. Sticky negative marker (or log) for Async-mode remove resurrection (6.5); document
    `reload()` as quiescence-required or snapshot-diff it (2.7, with 1.8's reload half);
    increment `size` only on successful insert (1.8 remainder).
20. Error taxonomy: `#[source]`/typed `Codec` variant; graceful `WriteWorker` spawn failure
    (6.8).
21. Docs sweep, one commit: hoist the thirteen function-local imports (7.1, with 1.10's
    import half); refresh `key_bytes.rs` module doc (7.2, with 5.5's half); delete the three
    Tests banners (7.3, with 5.8's bullet); import/share `RecordStream` and make it `pub`
    (7.4, with 5.8's bullet); drop the prefetch/phantom-engine rustdoc (5.5 remainder); fix
    the `:655` line ref (7.7); trim the retired-#535 narration (1.10 remainder); split the
    membuffer test mods into topic files (7.6); apply 5.8's remaining API-polish bullets.
22. Cosmetic batch-path allocations (`miss_keys.clone()`, hardcoded 256) (4.7).
