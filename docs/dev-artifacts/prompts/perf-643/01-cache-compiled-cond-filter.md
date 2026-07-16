# #643 — cache `$cond`'s compiled condition instead of recompiling per row

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The problem (already root-caused, measured, do not re-investigate)

`crates/shamir-engine/benches/cond_expr_eval.rs`'s doc-comment (read it in
full first — it already names the exact fix direction) measured `$cond`
evaluation inside `resolve_filter_query` at **~29x** (2-branch) to **~115x**
(3-level nested) the cost of a flat-literal baseline, over 1000
records/iter. Root cause, in `resolve_filter_query`
(`crates/shamir-engine/src/query/filter/resolve.rs`, `FilterValue::Cond`
arm):

```rust
FilterValue::Cond { cond } => {
    let node = compile_filter(&cond.condition, ctx.interner);
    if node.matches(record, ctx) {
        resolve_filter_query(&cond.then, record, ctx)
    } else {
        resolve_filter_query(&cond.or_else, record, ctx)
    }
}
```

`compile_filter` re-walks and re-interns `cond.condition: Box<Filter>`
(`crates/shamir-query-types/src/filter/cond.rs:41-50`) from scratch on
EVERY call — i.e. once per record in a per-row hot loop (the proven
production hot path is SELECT projection: see
`crates/shamir-engine/src/query/read/select_projection.rs`'s
`SelectProjection::project_value`, called once per matched record, which
evaluates any projected `FilterValue::FnCall`/`Cond` via
`resolve_filter_query`). But `cond.condition`'s `Filter` AST is STATIC per
query — identical on every call for the SAME `Cond` node — exactly like the
top-level WHERE filter, which IS already compiled once outside the
per-row loop (`SelectProjection`/read_exec's existing "compile once,
reuse per record" pattern — see `SelectProjection::new()`'s pre-interning
of field paths for the established precedent).

## Chosen fix direction (decided by the orchestrator — implement exactly this)

**Opt-in, zero-risk cache via `FilterContext`.** `resolve_filter_query` is
called from MANY contexts (WHERE, `when`, `for_each`'s `over`, write-value
resolution) — most of them one-off, not per-row hot loops. Do NOT change
`resolve_filter_query`'s behavior for those callers. Instead:

1. **Add a `CondCache` type** in `shamir-engine`'s filter module (e.g.
   `crates/shamir-engine/src/query/filter/cond_cache.rs`): a
   `TMap<usize, FilterNode>` (or plain `HashMap` if `TMap` doesn't fit
   here — check what's already imported in `resolve.rs`/`filter_node.rs`
   and match convention) keyed by `&*cond.condition as *const Filter as
   usize` (the raw pointer address of the boxed `Filter` AST). This is
   SAFE and stable ONLY because the cache is built once from an owned,
   never-cloned-per-row `FilterValue` tree (see step 2) — document this
   invariant clearly with a safety comment at the cache's construction
   site: "the FilterValue tree this cache was built from must outlive the
   cache and must never be cloned/moved after construction — pointer
   identity is the cache key".
2. **Add a recursive pre-scan function** that walks a `&FilterValue` tree
   (mirroring `resolve_filter_query`'s own dispatch structure — read it in
   full to get the recursion right) and, for every `FilterValue::Cond`
   node found (at ANY nesting depth — inside `FnCall` args, inside
   `Expr` operands, inside another `Cond`'s `then`/`or_else` branches),
   compiles `cond.condition` via `compile_filter(&cond.condition,
   interner)` and inserts `(pointer, compiled_node)` into the cache.
3. **Extend `FilterContext`** (`crates/shamir-engine/src/query/filter/eval_context.rs`)
   with a new OPTIONAL field: `pub cond_cache: Option<&'a CondCache>`
   (defaulting to `None` in `FilterContext::new`, exactly like `actor`/
   `scalars`/`params` already default), plus a builder method
   `with_cond_cache(mut self, cache: &'a CondCache) -> Self` (mirror
   `with_actor`/`with_scalars`'s existing builder pattern).
4. **Update `resolve_filter_query`'s `Cond` arm** to check the cache FIRST:
   ```rust
   FilterValue::Cond { cond } => {
       let node = match ctx.cond_cache.and_then(|c| c.get(&(&*cond.condition as *const Filter as usize))) {
           Some(cached) => cached.clone(), // or however FilterNode is cheaply reused — check if it's Clone or needs Arc
           None => compile_filter(&cond.condition, ctx.interner),
       };
       ...
   }
   ```
   Check whether `FilterNode` is `Clone` (it may contain `Regex`/`OnceLock`
   fields that don't clone cheaply — if so, wrap cached entries in `Arc<FilterNode>`
   instead of cloning, and adjust `FilterNode::matches`'s call site to take
   `&FilterNode` either way, which it likely already does). When
   `ctx.cond_cache` is `None` (every EXISTING caller, unchanged), behavior
   is IDENTICAL to today — zero risk to WHERE/`when`/`for_each`/write-value
   paths.
5. **Wire it into `SelectProjection`** (`select_projection.rs`): in
   `SelectProjection::new()`, after building `funcs: Vec<(String,
   FilterValue)>`, run the pre-scan (step 2) over every `FilterValue` in
   `funcs` and store the resulting `CondCache` as a new field (e.g.
   `funcs_cond_cache: CondCache`). In `project_value()`, build the
   `FilterContext` with `.with_cond_cache(&self.funcs_cond_cache)` instead
   of the current bare `FilterContext::new(interner, &self.empty_refs)`.
6. **Small additional fix, same bench, same task**: `eval_filter_expr`
   (wherever `$expr`'s `add`/`sub`/etc. dispatch lives — grep for it)
   allocates a fresh `Vec::with_capacity(expr.args.len())` per call; the
   bench's own doc-comment flags this as part of the ~190x `$expr` cost.
   Check if switching that allocation to a `SmallVec` (this codebase's
   established pattern — see `CompactPath` in `filter_node.rs` for the
   convention: inline up to N typical elements, spill to heap beyond) for
   the common small-arity case (2-4 args) is a safe, independent win — do
   this ONLY if it's a clean, low-risk change; if it requires bigger
   restructuring, leave it and note why in your summary.
7. **Re-run `cond_expr_eval.rs`** after the fix
   (`CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p
   shamir-engine --bench cond_expr_eval`, forward slashes only) and update
   its doc-comment header with the NEW measured numbers and an honest
   conclusion (if the cache doesn't fully close the gap — e.g. per-row
   `FilterContext` construction or other overhead remains — say so
   plainly; do not claim victory beyond what the numbers show). Check for
   and delete any stray `devrust.cargo-target*` directory created by the
   Git-Bash path-escaping bug — never commit it.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green
  (the existing `$cond`/`$expr` unit/e2e tests must still pass unchanged —
  this is a pure perf fix, zero behavior change).
- The re-run bench output captured and included in your summary.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.

## Out of scope

- Do NOT change `resolve_filter_query`'s behavior for callers that don't
  pass a `cond_cache` (WHERE, `when`, `for_each`, write-value resolution) —
  zero behavior/perf change there is a hard requirement, not a suggestion.
- Do NOT touch #634, #659 — separate tasks.
- If `$expr`'s allocation fix (step 6) turns out to need deeper
  restructuring than a drop-in `SmallVec` swap, skip it and say so — the
  `$cond` caching fix (steps 1-5) is the primary deliverable.
