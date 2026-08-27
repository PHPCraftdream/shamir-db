# shamir-storage -- Performance & O(x->0)

Reviewer: static read-only pass over `crates/shamir-storage/src/**` + `Cargo.toml`
(judged against CLAUDE.md's five pillars, primarily pillar 3 "O(x -> 0)" and
the unbounded-growth hazards it names). Scope of this report: performance only;
correctness / lock-discipline / style themes are covered by sibling reviewers.

## Summary

The disk-tier backends (fjall, membuffer, cached) show clear evidence of the
2026-07-06 audit fixes -- incremental cursor scans, zero-copy reads,
bounded write-worker channel -- but the same fixes were never carried into
`InMemoryStore`, whose `iter_stream`/`scan_prefix_stream` still eagerly
materialize the entire corpus (or the whole prefix match-set) before the first
yield. The trait-default `iter_range_stream_reverse` is a full collect-and-
reverse in RAM and three stores (`InMemoryStore`, `CachedStore`, and therefore
`MirroredStore`) rely on it, so DESC/top-K/max queries on those tiers pay
O(range) time *and* memory even when K=1. Two remaining unbounded-buffering
hazards in theme: `MemBufferStore::transact` force-drains the entire dirty
buffer per call, and `CachedStore`'s `WriteMode::Async` queue has no bound.
Test coverage (~93 tests incl. range-bounds/batching/reverse-order suites) is
strong on ordering correctness but asserts nothing about laziness or memory
bounds on these paths, which is exactly how findings 1-2 survived.

## Findings

### 1. Reverse range streams drain the ENTIRE range into RAM before reversing; not overridden by InMemory/Cached/Mirrored
**File:line:** `src/types.rs:376-384` (trait default) + `types.rs:391-412`
(`default_reverse`); missing overrides at `src/storage_in_memory.rs:103-258`
(implements `iter_range_stream` at :171 but no reverse) and
`src/storage_cached.rs:414-717` (no `iter_range_stream*` overrides at all);
inherited through `src/storage_mirrored.rs:431-439`.
**Severity:** high
**Issue:** The default `iter_range_stream_reverse` composes the forward range
stream with `default_reverse`, which `extend`s every batch into one `Vec`
before yielding ("Memory ~ N items" per its own doc). Consumers of
reverse order are precisely the early-exit workloads -- `lookup_last_k`,
`lookup_max`, `ORDER BY ... DESC LIMIT K` (named in the method's own doc) --
yet each pays a full-range drain plus an O(range) resident allocation even for
K=1. Because `scc::TreeIndex::Range` implements `DoubleEndedIterator`
(`next_back`, verified against scc 3.8 docs), InMemoryStore can drive it
natively, and `storage_fjall.rs:430-483` already proves the incremental
reverse-cursor pattern this crate prefers.
**Failure scenario:** A hybrid table (`MirroredStore`, primary = InMemoryStore)
or cached table with millions of sorted-index postings receives
`lookup_last_k(k=10)`/DESC page requests; each request clones and holds all
matching entries before returning one batch. Memory spikes scale with total
range size, latency is linear in N for constant-K reads, and concurrent such
reads multiply the transient allocations.
**Suggested fix:** Override `iter_range_stream_reverse` in `InMemoryStore` with
a per-batch guarded `.range(..).rev()` walk that seeks to the last key in-bound
and resumes downward past it (`Bound::Excluded(last)`) -- mirror the existing
`iter_range_stream` body (:184-231), replacing `range(lo..)`+forward with a
reverse seek (scc's `Range::next_back`). Have `CachedStore` do the same over
its TreeIndex cache (its `scan_prefix_stream`:587 already shows the exact
repeated-bounded-requery shape). Keep the trait default as documented fallback
only.

### 2. InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole corpus before the first yield
**File:line:** `src/storage_in_memory.rs:153-159` (`iter_stream`),
`:240-247` (`scan_prefix_stream`).
**Severity:** high (medium-high if the mirrors/hybrid tier is considered cold
path; posting-list rebuild/settle makes it hot there via `MirroredStore`)
**Issue:** Both methods `collect()` ALL matching `(key, value)` pairs into a
`Vec` while holding an epoch guard, then hand the vec to the stream, which just
drains it in batch_size chunks. This is exactly the eager-collect anti-pattern
audit `2026-07-06-perf-radical-o-notation` §1.3 removed from `CachedStore`
(`storage_cached.rs:521-526` documents the fix) and from fjall/membuffer -- but
it was never applied to the in-memory backend. A consumer wanting only the
first batch (`LIMIT`-style pulls, `copy_store`'s early error paths) still pays
O(N)/O(matches) clones + a single large allocation up front; `TreeIndex::iter`
under a fresh guard each round-trip would keep memory O(batch_size).
Note the SAME FILE's `iter_range_stream` (:171-232) implements the correct
short-lived-guard + resume-key incremental pattern -- the inconsistency is
within one impl block.
**Failure scenario:** `MirroredStore::new` hydration streams via
`mirror.iter_stream` (fine), but any later `scan_prefix_stream` over the hybrid
primary (e.g. `SortedIndexManager::rekey_postings` re-scans, index lookup
warming) allocates the full match-set twice transiently (collect vec +
per-batch drained vecs) regardless of consumer appetite; under concurrent
scans these snapshots compound.
**Suggested fix:** Convert both methods to the :184-231 pattern: open a guard,
collect up to `batch_size` items starting after a resume key (for prefix: lower
bound = max(resume, prefix), stop when a key exits the prefix), drop guard,
yield, repeat. Total work unchanged; peak memory drops to O(batch_size) and
early-exit consumers pay only what they drain.

### 3. MemBufferStore::transact drains the ENTIRE dirty buffer before every transact
**File:line:** `src/storage_membuffer.rs:1037` (`self.drain_all().await?` inside
`transact`; drain_all itself at :598-618 also snapshots with
`batch_size = usize::MAX`, :600/:612).
**Severity:** medium
**Issue:** Only op-touched keys need flushing before delegating to
`inner.transact` (a pending dirty value v1 for key k MUST land before the batch
writes v2 directly to inner, else the next drain revives stale v1). Instead the
code flushes every dirty entry in the buffer -- unrelated point-writes included
-- to disk synchronously inside each transact. That is write amplification
proportional to total unflushed traffic, not to |ops|: the same
read/write-triggered-drain class audit §2.3 (task #530) removed for scans ("a
full flush is no longer required just to read" -- yet a transact forces one).
Additionally `drain_all` calls `drain_once(usize::MAX)`, snapshotting the whole
dirty DashMap (keys + cloned values) into RAM in one shot rather than in
`flush_batch_size` chunks.
**Failure scenario:** A table behind `CachedStore -> MemBufferStore -> fjall`
mixes steady single-row `set`s (buffered, flushed on the 500 ms tick) with a
moderate tx rate calling `transact`. Every transact flushes whatever happens to
be pending -- potentially hundreds of MiB of accumulated buffered rows (64 MiB
default `max_bytes` worth of cache alongside) -- turning one small batch commit
into a full write-back of unrelated data, defeating the fsync batching the
buffer exists to provide, and stalling the commit for the drain duration.
**Suggested fix:** Pre-drain ONLY the keys appearing in `ops` (build the key
set, snapshot those dirty entries via `dirty.get`, apply their values with
`set_many`/`remove_many` targeting just those keys, then CAS-clean via the
same `remove_if(slot == snapshot)` discipline used at :1060-1081). Chunk the
flush-path snapshots in `drain_once(batch)` units so even legitimate full
drains don't spike resident memory to O(all dirty).

### 4. CachedStore WriteMode::Async uses an UNBOUNDED write-behind channel
**File:line:** `src/storage_cached.rs:242`
(`mpsc::unbounded_channel::<CacheWriteJob>()`); jobs carry owned values:
`CacheWriteJob::Set { key, value }` at :55, enqueued at :450/:503.
**Severity:** medium
**Issue:** Async-mode `set`/`remove` enqueue onto a single worker through an
unbounded channel with no high-watermark, cap, or admission signal.
`pending_writes` counts the backlog but nothing acts on growth; each queued
`Set` holds its full `Bytes` value in addition to the copy already upserted
into the cache at :437. One serialized worker draining at `inner`'s write rate
is the only relief. Contrast `storage_fjall.rs:85-93`, which deliberately chose
`sync_channel(1024)` for this identical pattern with the explicit rationale
"a pathological fan-out can't OOM the queue" -- the lesson was applied at the
disk worker but not at the cache wrapper sitting above it.
**Failure scenario:** Data-tier store in Async mode against a backing store
that slows (cold cache, compaction stall, network volume): sustained producer
rate > single-worker drain rate grows the queue without limit; memory rises by
two copies per pending op until OOM. No telemetry surfaces depth except
polling `pending_writes`.
**Suggested fix:** Bound the queue (e.g. `async_channel`-style bounded or
`tokio::sync::mpsc` with capacity ~ the fjall worker's 1024) and make
submitters await send (async-aware backpressure), or keep std channel but
route the rare-full case through `try_send` + async wait. Optionally expose a
high-watermark log/metric off the existing `pending_writes`.

### 5. Trait-default range filter scans PAST the upper bound forever
**File:line:** `src/types.rs:419-447` (`default_range_filter` loop keeps
consuming batches and filtering after keys exceed `end`).
**Severity:** medium
**Issue:** The input stream is contractually ascending (`Store::iter_stream`
ordering guarantee, types.rs:293-302), so once a key exceeds `end_inclusive`
every subsequent key does too -- but the filter keeps draining the stream to
the end, discarding everything. Any backend relying on this default pays
O(pos(end)..N) wasted traversal instead of stopping at the boundary. Concrete
victim in-crate: `CachedStore` has no `iter_range_stream` override, so
upper-bounded range/order queries on the cached tier run the default filter
over its incremental full-store cursor; `InMemoryStore` is unaffected (native
override). No correctness issue; pure O(x->0) miss.
**Failure scenario:** An upper-bounded `iter_range_stream(Some(start),
Some(end))` on a CachedStore covering the top slice of a large store walks and
clones-checks every key beyond `end` -- cost proportional to what lies ABOVE
the requested window's end, growing with corpus size for a fixed-size query.
**Suggested fix:** Track a `done: bool`; inside the filter closure return-based
exit isn't enough across element boundaries -- set `done` when the first
out-of-window key is seen (`k > end`), then break the outer batch loop instead
of pulling further batches.

### 6. FjallStore::submit blocks the async caller thread when the bounded queue fills
**File:line:** `src/storage_fjall.rs:199` (`tx.send(...)` -- blocking
`SyncSender::send` executed directly on the tokio task, channel bound 1024 at
:93; trade-off documented at :85-93 and :188-193).
**Severity:** low
**Issue:** When >1024 inserts/transacts are concurrently in flight, submitters
block a runtime worker thread synchronously (pillar-2 violation: blocking op
inside async context). The design comment argues steady-state never blocks;
true, but bulk parallel loaders can plausibly cross 1024 outstanding
`insert`s since every submitter parks until its own job completes.
**Failure scenario:** A 4096-task fan-in bulk import: 3072 tasks sit blocked on
`send` occupying tokio worker threads; remaining runtime capacity starves while
one write thread drains 1024-deep backlog, degrading unrelated tasks sharing
the runtime.
**Suggested fix:** Try `try_send` first; on `TrySendError::Full`, hop the
blocking send into `spawn_blocking` (SyncSender is Clone/Send) or switch to
`tokio::sync::mpsc::channel(1024)` consumed by a dedicated thread via a small
adapter -- preserves measured worker wins, removes the sync-block-in-async
case.

### 7. Minor allocations/clones on batched paths
**Files:** `src/storage_membuffer.rs:1245` (`miss_keys.clone()` -- extra
key-vector clone per `get_many` containing misses; keys are `RecordKey`
mostly inline so cheap, but avoidable by zipping `miss_idxs` against kept-key
references); `src/storage_fjall.rs:617` and `:682`
(`Vec::with_capacity(256)` hardcoded instead of `min(batch_size, 256)` --
over-allocates for small batches, reallocates once for larger ones).
**Severity:** nit
**Issue / fix:** Cosmetic constant-factor cleanups on batch paths; no behavioral
risk either way. Listed for completeness, not as debt demanding action.

## Notes (theme-relevant non-findings)

- `FjallStore::set`/`remove` double LSM lookups (`contains_key` + mutation) are
  a known, bench-adjudicated trade-off with a sanctioned flag-free fast path
  (`set_no_flag`/`remove_no_flag`, storage_fjall.rs:394-403/:585-593) -- not a
  finding.
- MemBuffer's dirty map intentionally duplicates pending values outside moka
  (documented at :143-154) to survive cache eviction; bounded-ish in practice
  by the flusher under healthy I/O. Its residual unboundedness under flush
  failure is acknowledged in-code (audit §2.2, flush_errors counter, :187-192)
  -- Finding 3 addresses the amplification side; the writer-side
  high-watermark question is adjacent and worth a deliberate decision someday,
  but I did not verify a production writer outpaces the notify-driven drain
  loop, so I'm not raising it as a finding.
- Tests (`src/tests/`, 93 tests) cover ordering/bounds/batching/reverse-merge
  semantics well, including overlay merges -- but none assert incremental
  yield behavior or memory bounds on the InMemoryStore streams or queue-depth
  caps, matching where findings 1/2/4 hid.
