Task: MEDIUM-HIGH performance — `IndexManager`'s posting-set cache
deep-clones the ENTIRE `BTreeSet<RecordId>` on every cache hit (audit
finding 1.5), and non-vector indexes universally return/intersect
`BTreeSet<RecordId>` even though postings are read from an already-sorted
prefix scan and could be a dense sorted slice instead (audit finding
3.2), `docs/dev-artifacts/audits/2026-07-06-perf-radical-o-notation.md`.

## Where — 1.5 (posting-cache deep clone, PRIMARY fix, low complexity)

- `crates/shamir-index/src/legacy/index_manager.rs:634-636` (confirm
  current lines): `return Ok((**cached).clone())` — a cache HIT still
  does a full node-by-node allocation clone of the ENTIRE
  `BTreeSet<RecordId>` on every equality-lookup. For a low-cardinality
  index (e.g. `status = 'active'` with 100k postings), every query pays
  100k tree-node allocations even though the whole point of the cache
  is to AVOID re-deriving the posting set from disk — the cache
  "accelerates" the disk scan but the clone itself costs O(|postings|).
- The MISS path (`:669`, confirm current line) ALSO clones when
  populating the cache — check whether this clone is genuinely
  necessary (e.g. one copy stored in the cache, one returned to the
  caller) or could ALSO be eliminated by returning the same `Arc`.

## Fix — 1.5

1. Change the posting-cache's stored/returned type from
   `BTreeSet<RecordId>` (behind whatever wrapper it currently uses) to
   `Arc<BTreeSet<RecordId>>` — per the audit's fix sketch. A cache HIT
   then returns `Arc::clone(cached)` (an atomic refcount bump, O(1)) —
   no more full-set node clone.
2. Update every CALLER of this cache lookup to work with the `Arc`
   wrapper instead of an owned `BTreeSet` — per the audit, callers in
   `read_exec` (or wherever the actual consuming code lives — grep for
   callers of this function) only ITERATE or INTERSECT the set, which
   works identically through an `Arc<BTreeSet<RecordId>>` deref (no
   behavior change needed at the call site beyond possibly an added
   `&*` deref or adjusting a type annotation).
3. Confirm the cache's INSERT/populate path (the miss path) also
   avoids an unnecessary clone where possible — e.g. build the
   `BTreeSet` once, wrap it in `Arc::new(...)`, insert the `Arc` into
   the cache AND return `Arc::clone` of the SAME `Arc` to the caller
   (rather than the caller getting one clone and the cache storing
   another independently-built copy).

## Where — 3.2 (structural: BTreeSet → sorted-slice representation)

- `crates/shamir-index/src/legacy/index_manager.rs:626-671` and
  `sorted_index_manager.rs` (`lookup_range` → `BTreeSet`, confirm
  current locations): ALL non-vector indexes currently return and
  intersect `BTreeSet<RecordId>` — a per-node allocation, cache-unfriendly
  traversal, and O(n log m) intersection with cache misses on every
  step. Per the audit: postings are, BY CONSTRUCTION, read from an
  already-SORTED prefix scan (the underlying storage iteration is
  ordered) — so building a `BTreeSet` from already-sorted data and then
  paying tree-traversal costs to re-read it is pure waste; a sorted
  `Vec`/slice representation would be both cheaper to build AND cheaper
  to intersect (two sorted slices merge in O(n+m) linear time via a
  galloping/merge-join, vastly better cache locality than tree
  pointer-chasing).

## Fix — 3.2 (attempt if tractable; may be scoped down — see below)

1. Investigate switching the canonical posting-list representation from
   `BTreeSet<RecordId>` to `Arc<[RecordId]>` (a sorted, immutable dense
   slice) — per the audit's fix sketch, combined with 1.5's `Arc`
   wrapping this becomes `Arc<[RecordId]>` directly (no need for a
   separate `BTreeSet` wrapper at all once the representation itself is
   a sorted slice).
2. Rework the intersection logic (AND across two indexes) from
   `BTreeSet` set-intersection to a linear two-sorted-slices merge/
   galloping-intersect (`RecordId` needs a total order for this — check
   it likely already has one, being backed by a fixed-size byte array).
3. Rework the union logic (OR across two indexes) analogously via
   k-way merge if that code path exists.
4. **Scope-down escape valve (use judgment given the size of this
   change)**: 3.2 is explicitly flagged by the audit as "структурное"
   (structural/architectural), touching the canonical representation
   across `index_manager.rs`, `sorted_index_manager.rs`, and any
   AND/OR/filter-combination logic elsewhere that consumes these
   posting sets. If, after investigating the actual blast radius (how
   many call sites/modules would need to change), this proves too large
   for a single surgical PERF task — **STOP the 3.2 portion, ship 1.5
   alone** (which is independently valuable and low-risk per the
   audit's own complexity rating: "Низкая" for item 3's Arc-wrap half,
   vs. "Низкая→средняя" for the combined item), and write up EXACTLY
   what 3.2 would require as a follow-up task description in your
   report (file list, call-site count, suggested incremental migration
   path) so it can be split into its own dedicated task rather than
   attempted piecemeal here. Do NOT half-migrate the representation
   (e.g. changing storage but not intersection logic, or vice versa) —
   either complete 3.2 cleanly or defer it entirely; a half-done
   representation change is worse than not starting.

## Performance verification requirement (MANDATORY — this is a PERF task)

1. Find or add a bench exercising the posting-cache HIT path
   specifically (repeated equality lookups against a populated,
   moderately-large posting set — e.g. 10k-100k RecordIds, matching the
   audit's "100k постингов" example) — measure lookup latency BEFORE
   (full clone) and AFTER (Arc clone) the 1.5 fix. Follow this repo's
   `bench-scale-tool::Harness` convention (see tasks #486/#487's
   `storage_fjall_pump.rs`/`storage_cached_pump.rs` for the current
   pattern to match — check `crates/shamir-index/benches/` for where a
   new bench belongs, or extend an existing one if a suitable posting-
   cache bench already exists, e.g. `sq8_hot_path.rs`'s sibling files).
2. If 3.2 is attempted, ALSO bench the intersection (AND) path with two
   indexes of comparable size, before/after.
3. Report exact baseline vs. after numbers with speedup ratios, per the
   `/opti` convention. If a fix shows no measurable improvement,
   investigate and report honestly why (matching the precedent set by
   tasks #486/#487 in this campaign).

## TDD/regression requirement

1. Confirm existing index-lookup/posting-cache tests still pass
   unchanged in BEHAVIOR (same query results) after the representation/
   wrapper change — this task must not alter query semantics, only
   internal representation and allocation cost.
2. Add a test confirming a cache HIT returns the SAME underlying data
   (via `Arc::ptr_eq` or equivalent, if feasible, to directly prove the
   no-clone claim) as a prior population, or at minimum confirms
   correctness (same `RecordId` set) after the change.
3. If 3.2 is attempted, add tests confirming sorted-slice intersection/
   union produce IDENTICAL results to the old `BTreeSet`-based logic
   for a range of overlap patterns (fully disjoint, fully overlapping,
   partial overlap, empty sets).

## Test scope command

```
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine -- index
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-index -- --check
cargo clippy -p shamir-index --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly (per this repo's `/opti` convention):
```
[Cycle: PERF-RADICAL-3]
  > Baseline:     <posting-cache-hit bench, before>
  > Изменения:    posting cache returns Arc<BTreeSet> instead of
                   deep-cloning on hit (1.5). [3.2: attempted/deferred — state which]
  > Тесты:        green / fixed N
  > After:        <same bench, after>
  > Δ:            <Nx>
```
- Confirm whether 3.2 (sorted-slice representation) was attempted,
  completed, or deliberately deferred — and if deferred, the exact
  scoped-down follow-up description (files, call-site count, suggested
  migration path) for a future dedicated task.
- Full test/gate results (exact commands + pass/fail).
