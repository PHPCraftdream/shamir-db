# F2 — hoist `$query`/`$param` per-row path parsing + navigation in `resolve_filter_query`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Second item of "Этап 8 — Performance" (post-blocker, не гейт релиза;
`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 07 (`docs/dev-artifacts/research/2026-07-17-release-audit/
07-performance-optimizations.md`), finding **F2** (severity **High**) —
read that section (lines 89-114) in full first.

This is the SECOND task in a sequence touching the same file
(`crates/shamir-engine/src/query/filter/resolve.rs`) — task 8a (F1, commit
`9ba703e8`) just landed a `FieldPathCache` for the `FieldRef` arm using a
prescan-at-compile-time pattern. **Read that commit's diff first**
(`git show 9ba703e8`) and read `crates/shamir-engine/src/query/filter/
field_path_cache.rs` in full — it establishes the style/naming/safety-comment
conventions this task should match. **This task is structurally DIFFERENT
from F1**, though — read the next section carefully before assuming the same
mechanism applies unchanged.

## The problem (already root-caused by the report, do not re-investigate)

`resolve_filter_query`'s `QueryRef` arm (`resolve.rs:219-222`):

```rust
FilterValue::QueryRef { alias, path } => {
    let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
    let qr = ctx.resolved_refs.get(key)?;
    resolve_query_ref_value(qr, path.as_deref())
}
```

`resolve_query_ref_value` (`resolve.rs:572-594`) → for the `value`-based
path (Call results), calls `resolve_query_value_path` (`resolve.rs:635-667`)
which does **string path parsing per row**: `rest.find(['.', '['])`,
`usize::parse`, prefix strips, one iteration per path segment — even though
`path: &Option<String>` is a literal string embedded in the SAME `FilterValue::
QueryRef` node on every row (static per query, exactly like `FieldRef`'s
`path` that F1 just fixed). The final `.cloned()` at the end of the walk
(`resolve.rs:575,590,593`) deep-clones the target `QueryValue` — this part is
NOT avoidable within this task's scope (see "What this fix does NOT remove"
below).

## Why this is NOT the same mechanism as F1 — read before implementing

F1's `FieldPathCache` worked because a `FieldRef`'s resolution (path →
interned keys) is **100% static per query** — no scan data is involved, so
the whole cache could be built once at `SelectProjection::new()` time
(before any row is read), via an eager prescan.

`$query`'s resolution is **NOT** static-at-compile-time in the same way:
`ctx.resolved_refs` (the actual referenced `QueryResult` data) only exists
once the sub-batch/read execution begins — it is NOT available at
`SelectProjection::new()`. What IS static per query is the **path string
itself** (`path: &Option<String>`) and, once the scan starts, the **resolved
target** is invariant across every row of that ONE scan (`ctx.resolved_refs`
is a fixed `&'a TMap<String, QueryResult>` reference for the whole scan).

This is the exact same shape the `In` node's `ref_column_sets` already
solves in-tree (`crates/shamir-engine/src/query/filter/filter_node.rs:280,
480-500` — **read this in full**, it is the correct template for THIS task,
not `CondCache`/`FieldPathCache`): a `OnceLock` **populated lazily on the
first row**, not eagerly at compile time, because the value it caches
depends on per-scan runtime data (`ctx.resolved_refs`) that doesn't exist
yet at compile time.

## Chosen fix direction (decided by the orchestrator — implement exactly this)

1. **Add a new file** `crates/shamir-engine/src/query/filter/
   query_ref_cache.rs` (one file = one primary export). Define:
   ```rust
   pub type QueryRefCache = TMap<usize, OnceLock<Option<QueryValue>>>;
   ```
   Keyed by pointer identity of the `QueryRef` node itself (`fv as *const
   FilterValue as usize`, same style as `FieldPathCache`). Each entry starts
   as an EMPTY `OnceLock` (no value yet — unlike `FieldPathCache`, we cannot
   populate the value at prescan time, only reserve the slot).
2. **Add a prescan function**, `prescan_query_ref_cache(fv: &FilterValue,
   cache: &mut QueryRefCache)`, mirroring `prescan_field_path_cache`'s
   recursion shape (same dispatch structure, same `prescan_filter` sibling
   walk for `Cond`'s condition operands) — but for `FilterValue::QueryRef`
   nodes, insert `(fv as *const FilterValue as usize, OnceLock::new())` —
   an EMPTY cell, not a resolved value (no `Interner`/`resolved_refs`
   parameter needed by this prescan, unlike F1's — note this signature
   difference explicitly in your implementation, don't carry an unused
   parameter over from the F1 template).
3. **Extend `FilterContext`** (`eval_context.rs`) with a new OPTIONAL field:
   `pub query_ref_cache: Option<&'a QueryRefCache>` (defaults `None`), plus
   `with_query_ref_cache(mut self, cache: &'a QueryRefCache) -> Self`.
4. **Update `resolve_filter_query`'s `QueryRef` arm** to use the cache:
   ```rust
   FilterValue::QueryRef { alias, path } => {
       if let Some(cell) = ctx
           .query_ref_cache
           .and_then(|c| c.get(&(fv as *const FilterValue as usize)))
       {
           return cell
               .get_or_init(|| {
                   let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
                   ctx.resolved_refs
                       .get(key)
                       .and_then(|qr| resolve_query_ref_value(qr, path.as_deref()))
               })
               .clone();
       }
       let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
       let qr = ctx.resolved_refs.get(key)?;
       resolve_query_ref_value(qr, path.as_deref())
   }
   ```
   Check this compiles against the actual match-arm binding names (confirm
   `fv` is the right variable bound at the outer `match fv { ... }` — same
   check F1's brief already flagged) and adjust as needed. When
   `ctx.query_ref_cache` is `None` (every existing caller, unchanged),
   behavior is IDENTICAL to today.
5. **Wire it into `SelectProjection`** the same way `funcs_field_path_cache`
   is wired (F1, just landed) — EXCEPT the prescan here does NOT need
   `interner` (see step 2) and the cache is filled lazily during
   `project_value` calls, not eagerly in `new()`. Add a
   `funcs_query_ref_cache: QueryRefCache` field, populate it via
   `prescan_query_ref_cache` in the SAME loop as the other two prescans in
   `new()`, and add `.with_query_ref_cache(&self.funcs_query_ref_cache)` to
   the `FilterContext` builder chain in `project_value()`.
6. **Investigate `$param` (the `FilterValue::Param` arm, `resolve.rs:234-239`)
   yourself before touching it** — the report groups it with `$query` as
   "likewise clones per row", but read `ctx.params`'s type
   (`&'a TMap<String, QueryValue>` in `eval_context.rs`) and the arm's
   actual body (`ctx.params.get(name.as_str()).cloned()`): this is ALREADY a
   single O(1) map lookup + one clone, with NO string path parsing and NO
   multi-step navigation — i.e., already the same shape this task is trying
   to REACH for `$query`, not a caller doing per-row re-parsing. If your own
   reading confirms this (it should), **do NOT add a cache for `Param`** —
   there is nothing to hoist; state this finding plainly in your summary
   instead of mechanically adding a no-op cache layer just because the
   report mentions it in the same breath. If you find a REAL per-row cost in
   the `Param` arm that this analysis missed, fix it and explain what you
   found — but the default expectation, stated here explicitly, is that
   `Param` needs no change.

## What this fix does NOT remove (be honest about this in your summary)

The final `.clone()` of the resolved target `QueryValue` still happens on
EVERY row (cache hit or miss) — `resolve_filter_query`'s public contract
returns `Option<QueryValue>` (owned) everywhere, and changing that to
`Option<Cow<QueryValue>>` or similar would ripple through `FnCall` arg
resolution, `Expr` operand resolution, `Cond` branch resolution, and `Array`
element resolution — a much larger, riskier refactor explicitly OUT OF
SCOPE for this task. What this fix DOES remove, on every row after the
first, per `QueryRef` node: the `alias.strip_prefix` + map lookup (cheap
already), the STRING PATH PARSING (`rest.find(['.', '['])`/`usize::parse`/
prefix strips, one pass per path segment), and the multi-step Map/List
navigation walk to locate the target value. Only the unavoidable final
`QueryValue::clone()` remains — state this tradeoff plainly, matching F1's
honesty precedent about what a cache mechanism can and cannot remove.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green
  — run it TWICE (this session's established discipline for perf-adjacent
  changes: a transient full-suite flake unrelated to your diff should
  reproduce as a PASS on a second run; if the SAME test fails twice,
  investigate it as a real regression, don't dismiss it).
- Add at least one new regression test proving the cache-HIT path
  (`with_query_ref_cache`) and the cache-MISS path (`field_path_cache`
  aside, just omit `.with_query_ref_cache(...)`) produce IDENTICAL results
  for the same `$query`-referencing filter, across at least two different
  `ctx.resolved_refs` scans (to prove the cache isn't stale/shared
  incorrectly across different `FilterContext` instances — each
  `SelectProjection`/scan should get its own fresh `QueryRefCache` built at
  `new()` time, never reused across unrelated queries).
- Add a test proving the OnceLock is populated lazily (empty before the
  first `resolve_filter_query` call on that node, populated after) — mirror
  how `ref_column_sets`' own behavior could be tested, or use the cache's
  public shape directly if `OnceLock::get()` is accessible for inspection in
  a test.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.

## Out of scope

- Do NOT touch F3/F4/F7 (HNSW/vector code) or F5 (ForEach) — separate
  tasks (8c, 8d, 8e).
- Do NOT touch `$param` unless your own investigation (step 6) finds a
  real, currently-missed per-row cost — the default expectation is no
  change needed there.
- Do NOT attempt to eliminate the final `QueryValue::clone()` by changing
  `resolve_filter_query`'s return type — explicitly out of scope, see "What
  this fix does NOT remove" above.
- Do NOT touch F1's `FieldPathCache`/`field_path_cache.rs` — already landed,
  read-only reference for this task.
