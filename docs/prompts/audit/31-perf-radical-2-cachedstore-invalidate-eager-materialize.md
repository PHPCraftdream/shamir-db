Task: HIGH performance — `CachedStore::transact` unconditionally
INVALIDATES (removes) cache entries for every key touched by a batch,
even for `Set` ops where the fresh value is known and could be cached
directly — making every read-after-write for freshly-committed data
systematically miss the cache and hit the (disk) backend (audit
finding, `docs/audits/2026-07-06-perf-radical-o-notation.md`, "read-after-write
×10-100" item). Also, `CachedStore::iter_stream`/`scan_prefix_stream`
eagerly collect ALL matching entries into a `Vec` before yielding the
FIRST batch, so a consumer with `LIMIT 10` still pays O(N) allocation/
clone cost (same audit doc, item near line 25/finding "жадная
материализация стримов").

## Where — read-after-write cache miss

- `crates/shamir-storage/src/storage_cached.rs`, `transact`
  (~line 326-345, confirm current lines):
  ```rust
  async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
      let keys: Vec<RecordKey> = ops.iter().map(|op| match op {
          KvOp::Set(k, _) | KvOp::Remove(k) => k.clone(),
      }).collect();
      self.inner.transact(ops).await?;
      for k in keys {
          if self.cache.remove(&k) {
              self.size.fetch_sub(1, Ordering::Relaxed);
          }
      }
      Ok(())
  }
  ```
  The `keys` collection captures ONLY the key for both `Set` and
  `Remove` variants — the VALUE from a `Set` op is discarded (via the
  `|` pattern-match arm merging both cases), even though it's fully
  available at the point `ops` is captured. Every key touched by
  `transact` is then unconditionally REMOVED from the cache
  post-commit, regardless of whether the op was a `Set` (where the
  fresh value could instead be POPULATED into the cache) or a `Remove`
  (where removal is correct).
  - Per the audit: the engine's commit pipeline writes batches of
    `KvOp::Set` (not `Remove`) for ordinary record writes — so this
    invalidate-instead-of-populate pattern means the cache is
    SYSTEMATICALLY cold on exactly the hottest keys (just-written
    data), forcing every read-after-write to hit the backend (disk)
    instead of RAM — a ×10-100 cost multiplier on that read, per the
    audit's estimate.

## Where — eager stream materialization

- `crates/shamir-storage/src/storage_cached.rs`, `iter_stream`
  (~line 273-295) and `scan_prefix_stream` (~line 297-321, confirm
  current lines): both collect the FULL matching entry set into a
  `Vec<(RecordKey, Bytes)>` (cloning every key AND value) BEFORE the
  `stream! { ... }` block starts yielding batches. A consumer that only
  wants the first `batch_size` (or fewer, via an upstream `LIMIT`) still
  pays O(N) clones + O(N) allocation for the FULL cache contents (or
  full prefix-matching subset) before seeing even the first row.
  - Per the audit's fix sketch: `scc::TreeIndex`'s underlying
    range/iter API can resume from a cursor position (the LAST key
    yielded) rather than requiring a single eagerly-collected pass —
    this is the SAME incremental/cursor pattern
    `storage_fjall.rs::iter_stream` already uses for its backend
    (confirm this by reading that function for the pattern to mirror).

## Fix — read-after-write

1. In `transact`, distinguish `Set` from `Remove` when deciding what to
   do post-commit:
   - For a `Set(key, value)` op: after `self.inner.transact(ops)`
     succeeds, POPULATE the cache with the fresh `(key, value)` pair
     (an insert/upsert into `self.cache`, adjusting `self.size`
     accordingly — mirror however `size` is tracked elsewhere in this
     file for a genuine insert, e.g. in `set`/`cache_upsert` if such a
     helper exists — check the file for an existing "insert into
     cache and bump size" pattern to reuse rather than duplicating
     logic).
   - For a `Remove(key)` op: keep the EXISTING behavior — remove from
     cache (this is already correct; do not change it).
2. Watch for a subtlety: `ops` is CONSUMED by `self.inner.transact(ops)`
   (moved), so you'll need to capture BOTH the key and (for `Set` ops)
   the value BEFORE that call, not just the key as the current code
   does — restructure the pre-collection step to build something like
   `Vec<(RecordKey, CacheAction)>` where `CacheAction` is
   `Populate(Bytes)` or `Invalidate`, built from the ORIGINAL `ops`
   slice before it's moved.
3. Confirm this doesn't break the cache's SIZE-BOUNDING logic (if
   `CachedStore` has a max-size/eviction policy — check for one in this
   file) — populating on `Set` should follow the SAME size-accounting
   discipline as any other cache-insert path in this file (e.g. does an
   insert ever trigger an eviction check? If so, the new "populate on
   Set" path must go through the identical accounting, not a
   parallel/duplicated one).
4. Consider (and report your decision on) whether a `transact` batch
   containing MANY `Set` ops should populate ALL of them into cache, or
   whether there's a risk of a single large batch blowing the cache's
   size bound — check whether the audit or existing code implies a cap
   on how much of a `transact` batch gets cache-populated, or whether
   "populate everything the batch touched" is the correct, simple
   behavior (matching how a `set`/single-key write path already
   populates cache on write). Default to matching the existing
   single-key `set` path's behavior for consistency unless there's a
   clear reason not to.

## Fix — eager stream materialization

1. Rework `iter_stream` and `scan_prefix_stream` to yield batches
   INCREMENTALLY rather than collecting the full result set upfront.
   Investigate `scc::TreeIndex`'s actual API (check the `scc` crate's
   docs/source for what iteration/range primitives it exposes — e.g.,
   can you get an iterator/cursor that's `Send` and can be driven batch-
   by-batch across `.await` points inside the `stream! {}` macro, or
   does `scc::ebr::Guard`'s lifetime make holding an iterator across a
   yield point awkward/unsound, requiring a different approach — e.g.,
   re-querying with `range(last_key_exclusive..)` for each batch,
   mirroring the "resume by last key" cursor pattern the audit says
   `storage_fjall.rs::iter_stream` already implements for its backend).
2. Read `storage_fjall.rs::iter_stream`'s actual implementation FIRST
   (it's cited by the audit as already doing the right thing) and
   mirror its cursor/resume-by-last-key shape for `storage_cached.rs`'s
   two functions, adapted to `scc::TreeIndex`'s API instead of fjall's.
3. If `scc::TreeIndex`'s EBR guard lifetime genuinely makes a fully
   lazy, held-cursor-across-yields stream infeasible or unsound (this
   is a real possibility given `scc`'s EBR (epoch-based reclamation)
   design — a `Guard` typically must not outlive a very short scope),
   the acceptable fallback is: fetch and yield ONE BATCH at a time via
   repeated bounded range queries (`range(last_key_exclusive..).take(batch_size)`),
   each within its own short-lived `Guard`, rather than one single
   upfront full collection. This still turns "always O(N) before the
   first byte" into "O(batch_size) before the first batch, then
   O(batch_size) per subsequent batch" — closing the audit's complaint
   even if a literal held-cursor stream isn't achievable with `scc`'s
   API. Investigate which approach is actually implementable and
   report which you used and why.
4. Confirm the resulting stream still yields entries in the SAME sorted
   order as before (TreeIndex iteration order) — do not introduce a
   correctness regression in ordering while fixing the eagerness.

## Performance verification requirement (MANDATORY — this is a PERF task)

Per this repo's `/opti` methodology:
1. Add or extend a bench exercising:
   - `transact` with a `Set`-heavy batch, immediately followed by a
     `get` on one of the just-written keys — this is the exact
     read-after-write scenario the audit describes. Measure this
     BEFORE (cache miss, hits backend) and AFTER (cache hit) the fix.
   - `iter_stream`/`scan_prefix_stream` with a `LIMIT`-like early-break
     (only consume the first batch or two from the stream, then stop)
     against a LARGE cache (e.g. 10k+ entries) — measure whether the
     BEFORE version pays full O(N) regardless of early consumer
     termination, and whether the AFTER version's cost scales with
     what was actually consumed instead.
   - Follow this repo's actual current bench convention (per task
     #486's finding: this repo uses `bench-scale-tool::Harness`, NOT
     Criteria/`shamir_bench_utils::tune` — check `crates/shamir-storage/benches/`
     for the current pattern, e.g. the newly-added `storage_fjall_pump.rs`
     from task #486, and match its structure).
2. Report exact baseline vs. after numbers, with the speedup ratio, in
   the `/opti` convention format.
3. If the eager-materialization fix's cursor/batched-requery approach
   doesn't show the expected improvement for early-terminated
   consumption, investigate and report honestly why (mirroring task
   #486's precedent of honest reporting even for flat results).

## TDD/regression requirement

1. Add tests confirming: after a `transact` with `Set` ops, a
   subsequent read of one of those keys is served from CACHE (not a
   backend hit) — if there's a way to observe this distinction in
   tests (e.g. a test double/mock inner store that panics or counts
   calls on backend reads, or an existing cache-hit counter/metric in
   `CachedStore`), use it; otherwise assert via the cache's own
   internal state (e.g. `cache.get(&key)` returns `Some` immediately
   after `transact`, without needing to prove it came from cache vs.
   backend via a mock).
2. Add tests confirming `Remove` ops in a `transact` batch still
   correctly evict the cache entry (regression guard — this behavior
   must NOT change).
3. Add or extend tests confirming `iter_stream`/`scan_prefix_stream`'s
   sorted-order and full-result correctness is preserved after the
   incrementalization (not just the perf characteristic).

## Test scope command

```
./scripts/test.sh -p shamir-storage
./scripts/test.sh -p shamir-engine
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly (per this repo's `/opti` convention):
```
[Cycle: PERF-RADICAL-2]
  > Baseline:     <read-after-write bench + stream-early-termination bench, before>
  > Изменения:    transact populates cache on Set instead of invalidating;
                   iter_stream/scan_prefix_stream made incremental/cursor-based
  > Тесты:        green / fixed N
  > After:        <same benches, after>
  > Δ:            <Nx for read-after-write, Nx for early-terminated stream consumption>
```
- Confirm the size-accounting/eviction discipline for the new
  populate-on-Set path matches the existing cache-insert discipline.
- Confirm which incrementalization strategy was used for the streams
  (held cursor vs. repeated bounded re-query) and why, given `scc`'s
  actual API constraints.
- Confirm sorted-order/correctness is unchanged.
- Full test/gate results (exact commands + pass/fail).
