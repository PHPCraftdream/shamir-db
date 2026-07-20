# F1 — cache `FieldRef`'s per-row interning in `resolve_filter_query`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

First item of "Этап 8 — Performance" (post-blocker, не гейт релиза;
`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 07 (`docs/dev-artifacts/research/2026-07-17-release-audit/
07-performance-optimizations.md`), finding **F1** (severity **High**) —
read that section (lines 45-87) in full first.

**The problem (already root-caused by the report, do not re-investigate):**
`resolve_filter_query`'s `FieldRef` arm
(`crates/shamir-engine/src/query/filter/resolve.rs:209-218`):

```rust
FilterValue::FieldRef { path } => {
    let keys = intern_field_path(path, ctx.interner)?;          // Vec alloc + 1 DashMap lookup PER SEGMENT
    let ipath: SmallVec<[InternerKey; 4]> =
        keys.iter().map(|&id| InternerKey::new(id)).collect();  // second buffer
    record
        .materialize_at(&ipath)
        .and_then(|iv| inner_value_to_query_value(&iv, ctx.interner).ok())
}
```

`intern_field_path` (`resolve.rs:48-55`) allocates a `Vec<u64>` and performs
one `Interner::get_ind` (a DashMap shard lookup) **per path segment, per
record** — even though the field path string (`path: &[String]`) is
identical on every call for the SAME `FieldRef` node in the SAME query. This
is exactly the shape task #643 already fixed for `$cond`'s condition
compilation, one layer down — read `crates/shamir-engine/src/query/filter/
cond_cache.rs` in full first, it is the established precedent for
EVERYTHING in this brief (pointer-keyed opt-in cache, `Option<&'a Cache>` in
`FilterContext` defaulting to `None`, zero behavior/perf change for callers
that don't opt in).

This `FieldRef` arm is reached from many contexts per report 07: `$fn` args
in projections (`SELECT upper(name)` re-interns `name` on every row via
`select_projection.rs:124-127` → `resolve_filter_query`), `$expr` operands
(`resolve.rs:281,312-315`), `$cond` `then`/`or_else` branches
(`resolve.rs:269,271`), and non-literal RHS of `Compare`/`Between`/
`Contains`/`In` in `filter_node.rs`.

## Chosen fix direction (decided by the orchestrator — implement exactly this)

**Opt-in, zero-risk cache via `FilterContext`, mirroring `CondCache`
exactly.**

1. **Add a new file** `crates/shamir-engine/src/query/filter/
   field_path_cache.rs` (one file = one primary export, per this repo's
   CLAUDE.md convention — do NOT add this to `cond_cache.rs`, which owns
   `CondCache` only). Define:
   ```rust
   pub type FieldPathCache = TMap<usize, SmallVec<[InternerKey; 4]>>;
   ```
   Key it by pointer identity of the `FieldRef` **node itself**:
   `fv as *const FilterValue as usize` (NOT the inline `path: Vec<String>`
   field — using the enclosing node's address is simpler and relies on the
   exact same invariant `CondCache` already documents: the tree this cache
   was built from must outlive the cache and never be cloned/moved after
   construction). Copy `CondCache`'s safety-comment style verbatim, adapted
   to this key choice — do not skip this, a future reader needs the same
   warning `cond_cache.rs` gives.
2. **Add a prescan function** in the same file,
   `prescan_field_path_cache(fv: &FilterValue, interner: &Interner, cache:
   &mut FieldPathCache)`, mirroring `prescan_cond_cache`'s recursion
   structure in `cond_cache.rs` (read it in full — same dispatch shape:
   recurse into `Array`/`FnCall`/`Expr`/`Cond`'s `then`/`or_else`, and into
   `Cond`'s `condition: Box<Filter>` via a sibling `prescan_filter`-style
   walk so `FieldRef`s nested inside comparison operands are also cached —
   `cond_cache.rs`'s `prescan_filter` is the template for that half). On a
   `FilterValue::FieldRef { path }` node: intern it ONCE via
   `intern_field_path` + the `SmallVec<InternerKey>` wrap (the exact two
   lines currently in `resolve.rs`'s `FieldRef` arm), and insert `(fv as
   *const FilterValue as usize, ipath)` into the cache — `or_insert_with`
   style, matching `CondCache`'s `entry(...).or_insert_with(...)`. If
   `intern_field_path` returns `None` (an unknown field name — can happen
   if the interner hasn't seen this string yet at prescan time, e.g. a
   brand-new field only ever referenced dynamically), skip the insert
   silently — the `FieldRef` arm's existing fallback path (see step 4)
   already handles a cache miss correctly, so this is not an error, just a
   soft miss exactly like `CondCache`'s own documented soft-miss behavior.
3. **Extend `FilterContext`**
   (`crates/shamir-engine/src/query/filter/eval_context.rs`) with a new
   OPTIONAL field: `pub field_path_cache: Option<&'a FieldPathCache>`
   (defaulting to `None` in `FilterContext::new`, exactly like `cond_cache`
   does), plus a builder method `with_field_path_cache(mut self, cache:
   &'a FieldPathCache) -> Self` (mirror `with_cond_cache` exactly).
4. **Update `resolve_filter_query`'s `FieldRef` arm**
   (`resolve.rs:209-218`) to check the cache first:
   ```rust
   FilterValue::FieldRef { path } => {
       let ipath: SmallVec<[InternerKey; 4]> = match ctx
           .field_path_cache
           .and_then(|c| c.get(&(fv as *const FilterValue as usize)))
       {
           Some(cached) => cached.clone(),
           None => {
               let keys = intern_field_path(path, ctx.interner)?;
               keys.iter().map(|&id| InternerKey::new(id)).collect()
           }
       };
       record
           .materialize_at(&ipath)
           .and_then(|iv| inner_value_to_query_value(&iv, ctx.interner).ok())
   }
   ```
   Check the exact variable name bound at the match site (the code binds
   `fv: &FilterValue` as the function parameter — confirm `fv` is in scope
   at this arm, since `match fv { FilterValue::FieldRef { path } => ... }`
   pattern-matches on it) and adjust the pointer expression accordingly.
   When `ctx.field_path_cache` is `None` (every EXISTING caller, unchanged —
   WHERE, `when`, `for_each`, write-value resolution, and any caller that
   hasn't opted in), behavior is IDENTICAL to today — zero risk.
5. **Wire it into `SelectProjection`**
   (`crates/shamir-engine/src/query/read/select_projection.rs`) the same
   way `funcs_cond_cache` is wired (read lines ~6, 40, 84-94, 130 first —
   this is the exact precedent to copy): in `SelectProjection::new()`, run
   `prescan_field_path_cache` over the same `funcs: Vec<(String,
   FilterValue)>` tree the existing `prescan_cond_cache` walks (same loop,
   same iteration — extend it, don't add a second loop), store the result
   as a new field `funcs_field_path_cache: FieldPathCache`, and in
   `project_value()` add `.with_field_path_cache(&self.funcs_field_path_cache)`
   to the `FilterContext` builder chain alongside the existing
   `.with_cond_cache(...)` call.
6. **Do NOT** touch any other caller of `resolve_filter_query` (WHERE
   compilation, `when`, `for_each`'s `over`, write-value resolution under
   `param_subst.rs`) — this brief is scoped to `SelectProjection`'s
   projection path only, exactly where `CondCache` was scoped. If you judge
   one of those other call sites would benefit equally and is a trivial,
   safe extension of the SAME mechanism (i.e. it already pre-scans a static
   tree once before a per-row loop, the same shape `SelectProjection` has),
   you may note that as a follow-up recommendation in your summary — but do
   NOT implement it in this task without flagging it first, since some of
   those callers may not have the "compile once, reuse per row" structure
   this cache requires and a caller-by-caller check is out of scope here.

## Measuring the fix (do this — do not skip)

There is no existing bench file targeting `resolve_filter_query`'s
`FieldRef` cost specifically. Before claiming the fix works:

1. Check whether `crates/shamir-engine/benches/` already has a bench file
   exercising a `SELECT $fn($ref)`-shaped projection (grep for
   `select_projection` / `FieldRef` / similar in
   `crates/shamir-engine/benches/*.rs`). If one exists, run it before and
   after your change
   (`CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p
   shamir-engine --bench <name>`, forward slashes only) and report the
   before/after numbers honestly — if the cache doesn't measurably help
   (e.g. because `materialize_at`/`inner_value_to_query_value` dominate the
   per-row cost, not the interning), say so plainly rather than claiming a
   win the numbers don't show.
2. If no such bench exists, do NOT create a full new Harness-based bench
   file for this alone (out of scope, this is a correctness-preserving perf
   fix, not a new benchmarked subsystem) — instead, describe in your
   summary what you'd expect the cache to remove (N segments × DashMap
   lookup + 1 Vec alloc, per row, per `FieldRef` node in a `SELECT $fn($ref)`
   projection) and note that no existing bench isolates this path.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green —
  the existing `SELECT $fn($ref)` / projection / `$cond`+`$ref` combination
  tests (including the 7 tests added in task 7d,
  `crates/shamir-engine/src/query/batch/tests/executor_tests/
  write_value_resolution_tests.rs`, and any `select_projection` tests) must
  still pass UNCHANGED — this is a pure perf fix, zero behavior change.
- Add at least one new regression test proving the cache path and the
  uncached (`field_path_cache: None`) path produce IDENTICAL results for
  the same `SELECT $fn($ref)`-shaped query — e.g. run the same projection
  through `SelectProjection` (cache populated) and through a raw
  `FilterContext::new(...)` call (cache `None`) and assert equal output.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above, plus the bench
  before/after numbers (or the honest "no existing bench isolates this"
  note per the Measuring section).

## Out of scope

- Do NOT implement F2 (`$query`/`$param` hoisting) — that is the next task
  in this sequence (8b), depends on reading THIS task's diff first since it
  touches the same `resolve.rs` file, and is deliberately kept separate so
  each commit stays reviewable on its own.
- Do NOT touch HNSW/vector code (F3/F4/F7) or ForEach (F5) — separate
  tasks.
- Do NOT touch any already-completed Этапы 1-7 artifacts.
- If you find `intern_field_path_compact` (`resolve.rs:62-72`, the
  `CompactPath = SmallVec<[u64;4]>` variant used by `filter_node.rs`'s
  compiled nodes) is a cleaner fit than plumbing through
  `SmallVec<InternerKey>` directly, you may use it as long as the final
  type stored in `FieldPathCache` still matches what `materialize_at`
  expects (`&SmallVec<[InternerKey; 4]>`) — a one-line `.map(InternerKey::new)`
  conversion at cache-population time is fine; just don't change
  `materialize_at`'s signature or `CompactPath`'s existing meaning in
  `filter_node.rs`.
