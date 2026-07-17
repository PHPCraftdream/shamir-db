# #665 — test coverage for `CondCache`'s cache-hit path (#643 gap)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

#643 added `crates/shamir-engine/src/query/filter/cond_cache.rs`: a
pointer-keyed cache (`CondCache = TMap<usize, Arc<FilterNode>>`) mapping a
`$cond`'s `condition: Box<Filter>` (by raw address) to its pre-compiled
`FilterNode`, so `resolve_filter_query`'s `FilterValue::Cond` arm
(`crates/shamir-engine/src/query/filter/resolve.rs:217-236`) can reuse a
compiled node across many records instead of recompiling
`cond.condition` on every row. `SelectProjection::new`
(`crates/shamir-engine/src/query/read/select_projection.rs`) is the one
production call site that populates this cache today, via
`prescan_cond_cache` walking every projected `FilterValue` once at
query-compile time.

**Zero test exists for any of this** — grepping the whole `shamir-engine`
crate for `cond_cache`/`CondCache` finds only the 5 non-test files that
implement/wire it (`cond_cache.rs`, `eval_context.rs`, `filter/mod.rs`,
`resolve.rs`, `select_projection.rs`). Specifically untested:

1. `prescan_cond_cache` itself — does it actually populate the cache with
   the right key for a `Cond`, and does it recurse into every shape that
   can embed a nested `$cond` (its own doc comment claims: `FnCall` args,
   `Expr` operands, `Cond` `then`/`or_else` branches, `Array` elements,
   AND — via the `prescan_filter` helper — `Filter`-embedded operands like
   `Eq.value`, `In.values`, `Between.from`/`to`, `ValueCompare.left`/
   `right`, `Computed.value`/`expr_args`)?
2. `cond_cache_get`'s pointer-identity lookup — hit for a registered
   condition, miss (`None`) for an unregistered one.
3. **The actual cache-HIT path inside `resolve_filter_query`'s `Cond` arm**
   (`resolve.rs:224-230`: `ctx.cond_cache.and_then(|c| cond_cache_get(c,
   &cond.condition))` then `Some(node) => node.matches(record, ctx)`) —
   does evaluating a `$cond` through a POPULATED cache actually produce
   correct, per-record-varying results (not a stale/fixed answer), proving
   the cached `FilterNode` is genuinely being reused and re-evaluated per
   record rather than baked to one answer at cache-population time?
4. The documented fallback (`ctx.cond_cache == None`, or a cache populated
   for OTHER `Cond` nodes but not this one) — does it still produce
   correct results via the `compile_filter` fallback branch?

This is exactly the class of gap a performance optimization can hide: the
cache could be silently wrong (e.g. caching against the wrong
record/producing the SAME answer regardless of which record is passed)
and no test would catch it, because nothing currently exercises the
hit path at all.

## What to add

### Part A — direct unit tests for `cond_cache.rs`

New file: `crates/shamir-engine/src/query/filter/tests/cond_cache_tests.rs`
(wire it into `crates/shamir-engine/src/query/filter/tests/mod.rs` —
`mod cond_cache_tests;` — following the existing `eval_tests`/
`filter_tests` pattern in that same `mod.rs`).

1. **`prescan_populates_simple_cond`**: build a `FilterValue::Cond` with a
   trivial condition (e.g. `Filter::IsNotNull { field: vec!["x".into()] }`
   — no real record needed for this structural check), call
   `prescan_cond_cache` into a fresh `CondCache`, then assert
   `cond_cache_get(&cache, &cond.condition)` returns `Some(..)` — the
   simplest possible cache-population check.
2. **`cond_cache_get_misses_unregistered_condition`**: build TWO separate,
   independent `Cond`s (distinct `Box<Filter>` allocations, so distinct
   pointer addresses). Prescan only the first into the cache. Assert
   `cond_cache_get(&cache, &second.condition)` returns `None` — the
   pointer-identity miss path.
3. **`prescan_recurses_into_all_documented_shapes`** (or split into
   several smaller tests, whichever reads better — table-driven with a
   `Vec<(&str, FilterValue)>` of labeled cases is fine): for EACH of the
   following, build a `FilterValue` tree where a `$cond` is nested at that
   position, run `prescan_cond_cache`, and assert the nested `Cond`'s
   condition IS found via `cond_cache_get`:
   - Inside `FilterValue::Array` (an array element is a `Cond`).
   - Inside `FilterValue::FnCall`'s args.
   - Inside `FilterValue::Expr`'s args.
   - Inside a `Cond`'s OWN `then` branch (a `$cond` whose `then` is itself
     another `$cond`).
   - Inside a `Cond`'s OWN `or_else` branch.
   - Inside the condition `Filter` itself, at an embedded `FilterValue`
     operand — e.g. `Filter::Eq { field, value: <nested Cond as FilterValue> }`
     as the OUTER `$cond`'s condition (exercises `prescan_filter`'s walk).
     Cover at least one more `prescan_filter` arm too (e.g.
     `Filter::In { values: [<nested Cond>], .. }` or
     `Filter::ValueCompare { left: <nested Cond>, .. }`) to prove the walk
     isn't special-cased to just `Eq`.
4. **`repeated_prescan_of_same_condition_is_idempotent`**: call
   `prescan_cond_cache` TWICE on the same `FilterValue` tree into the same
   cache (mirrors `cache.entry(key).or_insert_with(..)`'s intent) — assert
   the cache still has exactly one entry for that condition's pointer and
   `cond_cache_get` still resolves it correctly. (Guards the `or_insert_with`
   idempotency the doc comment implies but never tests.)

### Part B — prove the cache-HIT path produces correct, per-record results

This is the decisive test the gap is really about. Add to Part A's new
file (or a second test in the same file):

5. **`cached_cond_evaluates_correctly_per_record_not_stale`**: build a
   SINGLE `FilterValue::Cond` whose condition is a REAL record-field-based
   filter (e.g. `Filter::Gt { field: vec!["score".into()], value:
   FilterValue::Int(50) }`, `then: FilterValue::String("high")`, `else:
   FilterValue::String("low")`). Prescan it into a `CondCache` ONCE.
   Build a `FilterContext` via `.with_cond_cache(&cache)` (check
   `eval_context.rs`'s exact builder method name). Then call
   `resolve_filter_query(&cond_as_filter_value, record, &ctx)` for AT
   LEAST TWO records with DIFFERING `score` values straddling the
   threshold (e.g. `score: 80` and `score: 20`), using a real interned
   record (mirror `select_projection_tests.rs`'s `make_record` helper —
   intern field names via `interner.touch_ind`, build an
   `InnerValue::Map`). Assert the FIRST record resolves to `"high"` and
   the SECOND resolves to `"low"` — proving the SAME cached `FilterNode`
   produces genuinely different, per-record-correct answers (not a
   frozen/stale result from whichever record happened to populate the
   cache).
6. **`uncached_cond_still_resolves_correctly_via_fallback`**: same
   condition/records as test 5, but build the `FilterContext` WITHOUT
   `.with_cond_cache(..)` (the default, `cond_cache: None`) — assert the
   SAME correct high/low results via the `compile_filter` fallback branch.
   This is the regression pairing: cache-populated and cache-absent paths
   must agree.
7. **`cache_populated_for_other_conditions_still_falls_back_correctly`**:
   build TWO independent `Cond`s with DIFFERENT conditions/pointers.
   Populate the cache with ONLY the first. Evaluate the SECOND (uncached)
   `Cond` via `resolve_filter_query` with a `FilterContext` that carries
   the (irrelevantly-populated) cache — assert it still resolves
   correctly via the fallback branch (`cond_cache_get` returns `None` for
   THIS condition's pointer, `compile_filter` runs). This is the
   "populated cache, but a miss for the FilterNode actually asked about"
   case — distinct from test 6's "no cache at all".

### Part C — engine-level integration test through the real production seam

Add to `crates/shamir-engine/src/query/read/tests/select_projection_tests.rs`
(reuse its existing `make_record` helper):

8. **`project_value_cond_function_projection_caches_and_evaluates_per_record`**:
   build a `Select` with one `SelectItem::Function` whose `args` embed a
   `FilterValue::Cond` (e.g. a function named anything reasonable —
   check what `SelectProjection::new`'s `SelectItem::Function` handling
   actually requires for `name`/dispatch; if the scalar-fn dispatch layer
   requires a REAL registered function name to resolve without erroring,
   either pick an existing simple funclib scalar name whose first arg can
   be the `$cond`, or — simpler — skip the `FnCall` wrapper entirely and
   directly exercise `resolve_filter_query`/`SelectProjection`'s cache via
   whatever mechanism `funcs: Vec<(String, FilterValue)>` actually allows;
   read `SelectProjection::new`'s `SelectItem::Function` arm carefully
   first and use the most direct construction that gets a `Cond` into
   `self.funcs` without fighting scalar-fn dispatch unnecessarily).
   Build `SelectProjection::new(&select, &interner)` ONCE (this is what
   populates `funcs_cond_cache` via `prescan_cond_cache` internally — the
   real production call site). Then call `project_value` for AT LEAST TWO
   records with differing field values that drive the embedded `$cond`'s
   condition to different outcomes, on the SAME `SelectProjection`
   instance (proving the SAME internal `funcs_cond_cache` — built once —
   correctly serves both calls with per-record-correct answers). Assert
   the projected output differs correctly between the two records.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including every new
  test above.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Explicitly confirm (in your own words) that test 5/8 would have caught
  a hypothetical regression where the cache-hit branch returned a frozen
  answer instead of re-evaluating `node.matches(record, ctx)` per call —
  i.e. that the two records' differing expected outputs are load-bearing,
  not incidental.

## Out of scope

- Do NOT touch #666/#667 — separate tasks.
- Do NOT change `cond_cache.rs`/`resolve.rs`/`select_projection.rs`'s
  actual implementation — this task is test-coverage only. If you
  discover an ACTUAL bug while writing these tests (the cache producing
  wrong results), STOP, do not silently fix it, and report the finding in
  your summary instead — that would be a new, separate bug needing its
  own task, not something to fold into this test-only brief.
