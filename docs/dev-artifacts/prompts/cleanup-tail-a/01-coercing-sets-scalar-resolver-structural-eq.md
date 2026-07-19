# Cleanup tail A — coercing set-probes, ScalarResolver threading, Set/Map structural equality

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

This brief covers THREE independent findings (7, 8, 9) from a read-only
release audit (`docs/dev-artifacts/research/2026-07-17-release-audit/04-logical-correctness-bugs.md`).
Each is a small, well-scoped correctness fix — read the "Correct behavior"
sections below carefully, they were derived from reading the actual code.

---

## Fix 1 (Finding 7) — literal fast-path membership sets drop Int↔F64 coercion

### The bug

`crates/shamir-engine/src/query/filter/filter_node.rs`:

- `FilterNode::InSet` matches arm (~lines 362-385): `values.contains(&qv)` —
  exact `TSet<QueryValue>::contains`, no Int↔F64 coercion.
- `FilterNode::ContainsAnySet` matches arm (~lines 555-571):
  `list.iter().any(|item| values.contains(item))` and
  `set.iter().any(|item| values.contains(item))` — same exact-contains
  issue, per list/set element.
- `FilterNode::ContainsAllSet` matches arm (~lines 601-646): uses a cloned
  scratch `TSet` (`remaining`) and `remaining.swap_remove(item)` per
  field element — same exact-match issue.

The slow paths (`FilterNode::In`'s dynamic branch, `ContainsAll`) go through
`scalar_ref_cmp_qv`/`compare_values`, which treat `Int(1)` and `F64(1.0)` as
equal. The all-literal fast paths above use exact `TSet::contains`/
`swap_remove`, so `{"n": {"$in": [1.0]}}` against an Int field `n = 1` does
NOT match, while `{"n": {"$in": [1.0, {"$param": "x"}]}}` (any non-literal
element forces the slow path) DOES match the same row. A filter's answer
must not depend on whether its value list happens to be fully literal.

There is already a working precedent in the SAME file: `set_contains_coercing`
(~lines 48-76) — probes both `Int(n)` and `F64(n as f64)` (or vice versa) to
preserve `scalar_ref_cmp_qv`'s coercion semantics with two O(1) set lookups.
It is used today by `FilterNode::In`'s dynamic column-ref-set branch
(~line 452). Its own doc comment takes a `ScalarRef<'_>` — **read it in
full first**.

### The fix

`set_contains_coercing` takes a `ScalarRef<'_>`, but `InSet`/
`ContainsAnySet`/`ContainsAllSet` all work with fully-materialized
`QueryValue`s (via `materialize_at` + `inner_value_to_query_value`), not
`ScalarRef`. Add a `QueryValue`-native sibling with the IDENTICAL coercion
rules (same-type probe, then the opposite-type probe only when the value is
exactly representable in the other type) — e.g. `set_contains_coercing_qv(set:
&TSet<QueryValue>, qv: &QueryValue) -> bool` — and use it at all three call
sites in place of the exact `contains`/`swap_remove`:

- `InSet`: `values.contains(&qv)` → `set_contains_coercing_qv(values, &qv)`.
- `ContainsAnySet`: each `values.contains(item)` probe (both the List and Set
  arms) → `set_contains_coercing_qv(values, item)`.
- `ContainsAllSet`: `remaining.swap_remove(item)` needs to remove whichever
  of the two coercion-equivalent representations is actually present in
  `remaining` (probe both `Int`/`F64` forms of `item`, `swap_remove` whichever
  one is found — do NOT just check membership without removing, `remaining`
  must shrink or the O(field_len) early-exit and the "all required values
  found" final check both break).

Preserve the existing O(1)-per-probe / O(field_len)-total complexity — no
per-record allocation beyond what's already there (the `ContainsAllSet`
scratch clone already exists, don't add a second one).

**Do NOT touch `FilterNode::In`'s existing dynamic-branch behavior** — it
already has correct coercion via `set_contains_coercing`; this fix only
extends the SAME semantics to the three fast-path nodes above, closing the
gap the file's own comment at ~line 448 acknowledges ("known pre-existing
difference").

### Tests

1. `InSet` (all-literal `$in`) with an Int field and an F64 literal in the
   list (e.g. field `n = 1`, `{"$in": [1.0]}`) — must MATCH.
2. Same in reverse (F64 field, Int literal).
3. `ContainsAnySet` (all-literal `$contains_any`) — field is a `List`/`Set`
   containing `Int(1)`, literal set is `{1.0}` — must MATCH.
4. `ContainsAllSet` (all-literal `$contains_all`) — field contains
   `[Int(1), Int(2)]`, required literals `{1.0, 2.0}` — must MATCH (exercises
   the `swap_remove`-must-actually-remove case).
5. Regression: a `$contains_all` case from the existing bugfix-contains-all
   test suite (duplicate-counting fix, task 1b) must still pass unchanged —
   re-run `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs`
   and confirm no regression.
6. Regression: non-numeric types (Str/Bool/Bin) through all three nodes
   continue to use exact matching (no accidental coercion introduced for
   non-numeric types).

---

## Fix 2 (Finding 8) — user-registered scalars unavailable outside WHERE

### The bug

`FilterContext::new` (`crates/shamir-engine/src/query/filter/eval_context.rs:58-68`)
defaults `scalars` to `ScalarResolver::builtins_only()`. The WHERE path
threads the real per-DB resolver via `.with_scalars(...)`
(`crates/shamir-engine/src/query/batch/query_runner.rs:819-822`, look for
`self.resolver.scalar_resolver()`). Five OTHER contexts build a
`FilterContext` and do NOT call `.with_scalars(...)`, so a `$fn` referencing
a user-registered scalar silently evaluates as if the scalar doesn't exist
(builtins-only fallback — no error, no warning):

1. **`when` guard** — `crates/shamir-engine/src/query/batch/query_runner.rs`
   ~line 157: `FilterContext::new(&scratch, resolved_refs).with_actor(...).with_params(...)`.
2. **Sub-batch `bind`** — same file, ~line 348: same pattern.
3. **`for_each`'s `over`** — same file, ~line 500: same pattern.
4. **Group-SELECT scalar functions** —
   `crates/shamir-engine/src/query/read/aggregate.rs` ~line 801, inside
   `apply_group_by`: `FilterContext::new(interner, &empty_refs)` — no
   `.with_actor`/`.with_scalars` at all.
5. **SELECT projection scalar functions** —
   `crates/shamir-engine/src/query/read/select_projection.rs` ~line 122,
   inside `SelectProjection::project_value`: `FilterContext::new(interner,
   &self.empty_refs).with_cond_cache(...)` — no `.with_scalars`.

### The fix — sites 1-3 (query_runner.rs, easy)

`QueryRunner` already has `self.resolver.scalar_resolver()` in scope (used
at line 821). Add `.with_scalars(self.resolver.scalar_resolver())` to the
three `FilterContext::new(...)` builder chains at ~lines 157-159, 348-350,
500-502. Mirror the exact builder-chain style already used at line
819-822.

### The fix — site 4 (aggregate.rs, easy: `ctx` is ALREADY a parameter)

`apply_group_by` (`crates/shamir-engine/src/query/read/aggregate.rs:899-905`)
already takes `ctx: &FilterContext<'_>` as a parameter (the caller,
`crates/shamir-engine/src/table/read_exec.rs:827` and its siblings in
`read_temporal.rs`/`read_index_scan.rs`, already pass the real WHERE
context, which DOES carry the real resolver via query_runner.rs's own
construction). At ~line 801, instead of building a fresh builtins-only
`FilterContext::new(interner, &empty_refs)`, clone the resolver OFF the
already-available `ctx` parameter:
`FilterContext::new(interner, &empty_refs).with_scalars(ctx.scalars.clone())`
(confirm `ScalarResolver` is cheaply `Clone` — check its definition in
`crates/shamir-funclib/src/scalar_resolver.rs`; if it wraps an `Arc` this is
O(1)). No function signature change needed here — `ctx` is already in
scope.

### The fix — site 5 (select_projection.rs, requires a real signature change)

`SelectProjection` (`crates/shamir-engine/src/query/read/select_projection.rs`)
is built ONCE per query via `SelectProjection::new(select, interner)` (no
resolver parameter today) and reused per-record via
`project_value(record, interner)` (also no resolver parameter). Making
site 5 correct requires:

1. Add a `scalars: ScalarResolver` field to the `SelectProjection` struct
   (stored once, mirroring how `funcs_cond_cache` is already stored once).
2. Add a `scalars: ScalarResolver` parameter to `SelectProjection::new(...)`
   (read the whole function first — decide the clearest parameter position;
   check whether a `&ScalarResolver` reference is enough or the struct truly
   needs an owned clone given the struct's lifetime story).
3. In `project_value`, change `FilterContext::new(interner, &self.empty_refs)`
   to also call `.with_scalars(self.scalars.clone())` (or equivalent, matching
   whatever storage shape you chose in step 1).
4. Update EVERY production call site of `SelectProjection::new` to pass the
   real resolver. Find them all with
   `grep -rn "SelectProjection::new" crates/shamir-engine/src --include=*.rs`
   (excludes `tests/` and `benches/` — those are listed separately below).
   As of this brief, the production (non-test, non-bench) call sites are all
   in `crates/shamir-engine/src/table/read_exec.rs` (7 call sites) and
   `crates/shamir-engine/src/query/read/exec.rs` (1 call site) — **verify
   this list yourself, do not trust it blindly, the audit read may be
   stale.** At each site, a `ctx: &FilterContext` (carrying the real
   resolver via query_runner.rs's construction) is already in scope in the
   enclosing function — pass `ctx.scalars.clone()` through.
5. Update every TEST call site of `SelectProjection::new`
   (`crates/shamir-engine/src/query/read/tests/select_projection_tests.rs`,
   `crates/shamir-engine/src/table/tests/s3_bytes_path_parity_tests.rs`,
   `crates/shamir-engine/src/table/tests/recordview_cutover_parity_tests.rs`)
   to pass `ScalarResolver::builtins_only()` (preserving today's behavior in
   tests that don't care about user scalars) — EXCEPT any new test you add
   for this fix, which should pass a resolver with a registered test scalar
   to prove the wiring actually works end-to-end.
6. Update the benchmark call site(s) in `crates/shamir-engine/benches/`
   (`cond_expr_eval.rs` and any other bench constructing `SelectProjection`)
   the same way — `ScalarResolver::builtins_only()` is fine there, benches
   aren't testing scalar resolution.

**Do NOT change `FilterContext`'s own API** (`with_scalars` already exists
and is correct) — this fix is purely about THREADING the already-correct
mechanism into the 5 places that currently skip it.

### Tests

1. For EACH of the 5 sites: a regression test proving a user-registered
   scalar (register one via whatever test harness this codebase already uses
   for user scalars — grep `crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs`
   for the pattern) is now resolvable through that specific context, where
   it previously silently fell back to builtins-only. At minimum:
   - `when` guard referencing a user scalar evaluates correctly (not
     silently skipped).
   - Sub-batch `bind` resolving a user scalar.
   - `for_each`'s `over` resolving a user scalar.
   - A group-SELECT scalar-function projection resolving a user scalar.
   - A plain SELECT projection scalar-function resolving a user scalar.
2. Regression: existing `SelectProjection`/`apply_group_by`/`query_runner`
   tests continue to pass with builtins-only scalars (no behavior change
   for the builtins-only case).

---

## Fix 3 (Finding 9) — `count_distinct`/`mode` wrong over Set/Map values

### The bug

`crates/shamir-funclib/src/compare.rs`'s `compare` function (~line 49-69,
the module doc at the top already documents this as "intentionally loose")
compares `QueryValue::Set`/`QueryValue::Map` **by `.len()` only**
(~lines 64-65). `CountDistinctAgg` (`crates/shamir-funclib/src/agg.rs:191-215`)
and `ModeAgg` (~lines 727-767) both use `compare::compare(...) ==
Ordering::Equal` as their sole equality test — so two DIFFERENT single-entry
maps (`{"a": 1}` vs `{"b": 2}`) compare Equal, making `count_distinct` return
1 instead of 2, and letting `mode` report a value that appears once as "the
mode" when run-length counting merges it with an unrelated same-length
container.

### The fix

Fix this ENTIRELY in `compare.rs` — since both `CountDistinctAgg` and
`ModeAgg` delegate to `compare::compare`, fixing the comparator there fixes
both call sites with NO changes needed in `agg.rs`.

`QueryValue::Set` is `TSet<QueryValue>` and `QueryValue::Map` is
`TMap<String, QueryValue>` — both insertion-ordered (NOT sorted), so two
structurally-equal containers built in different insertion order must still
compare Equal, and `ModeAgg`'s `sort_by(compare::compare)` needs a
genuinely consistent total order (reflexive/transitive/antisymmetric) for
its run-length counting to be correct — length-only comparison happens to
already be consistent (a valid weak order), so any replacement must
preserve totality and transitivity too.

Approach (mirror `compare_lists`'s existing recursive element-wise pattern,
~lines 155-163, but canonicalize first since Set/Map are unordered — List
is NOT canonicalized because it's already ordered):

- **Map**: build a canonical `Vec<(&String, &QueryValue)>` for each side by
  sorting entries by key (String has a natural `Ord`), then compare
  element-wise: first by key (`Ord for String`), then — on equal keys — by
  recursively calling `compare` on the values. Unequal lengths after the
  common prefix: shorter is `Less` (mirror `compare_lists`'s tail exactly).
- **Set**: build a canonical `Vec<&QueryValue>` for each side by sorting
  elements using `compare` itself as the sort comparator (recursive — this
  is fine, `QueryValue` nesting is bounded in practice), then compare
  element-wise via recursive `compare`, same tail-length rule as Map/List.

Both canonicalizations are O(n log n) per comparison — acceptable here since
`compare` on Set/Map is already a cold path (aggregates over container
columns are not a hot path), same tradeoff already accepted by the
`.clone()`s in `ContainsAllSet`'s coercion fix above.

**Do NOT change `agg.rs`** — no code there needs to change; verify this by
running the funclib agg tests after your `compare.rs` change and confirming
`count_distinct`/`mode` now behave correctly with ZERO edits to
`CountDistinctAgg`/`ModeAgg`.

### Tests

1. `compare::compare` directly: two structurally-DIFFERENT single-entry Maps
   of equal length → NOT `Equal` (some consistent non-equal ordering).
2. `compare::compare` directly: two structurally-IDENTICAL Maps built with
   entries inserted in a DIFFERENT order → `Equal`.
3. Same two cases for `Set`.
4. Regression: `compare::compare` on two structurally-identical Lists still
   behaves exactly as before (List path untouched).
5. `count_distinct` (via `crates/shamir-funclib/src/agg/tests/agg_tests.rs`
   or wherever the existing agg tests live) over `[{"a":1}, {"b":2}]` →
   returns `2` (was `1` before the fix).
6. `mode` over `[{"a":1}, {"b":2}, {"b":2}]` → returns `{"b":2}` (the value
   that actually appears twice), not whichever the length-based run-length
   counting previously happened to merge.
7. Confirm the total-order requirement holds under a small property-style
   check if this codebase has a convenient way to do so (e.g. sort a mixed
   Vec of several Sets/Maps via `compare` and assert the sort is stable/
   consistent across repeated runs) — if that's overkill for this codebase's
   test conventions, a handful of explicit 3-way comparisons (transitivity:
   if `a < b` and `b < c` then `a < c`) covering Set and Map is sufficient.

---

## Verification (MANDATORY before you report done, for ALL THREE fixes)

- `./scripts/test.sh @engine --full` green, including all new/modified
  tests (covers Fixes 1 and 2).
- `./scripts/test.sh -p shamir-funclib --full` green (covers Fix 3).
- `cargo fmt --all -- --check` clean (or scoped to touched crates, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) Fix 1 does not change `FilterNode::In`'s existing
  dynamic-branch coercion behavior, only extends the same semantics to the
  three all-literal fast-path nodes; (b) Fix 2 lists every production call
  site of `SelectProjection::new` you found and updated (the count may
  differ from this brief's estimate — report the real number); (c) Fix 3
  required zero changes to `agg.rs`, or explain why if it turned out
  otherwise.

## Out of scope

- Do NOT touch `FilterNode::In`'s existing `set_contains_coercing` /
  `ScalarRef`-based helper — Fix 1 adds a QueryValue-native SIBLING, it does
  not modify the existing one.
- Do NOT implement self-referential FK enforcement or FK Int↔F64 child-
  matching coercion — those are task 3d (`docs/dev-artifacts/prompts/`
  brief not yet written), a separate follow-up.
- Do NOT touch checked-arithmetic issues (`$expr mod`, Sum accumulator
  overflow, `diff_secs`, funclib `compare`'s Int↔Big f64 comparison,
  `cast_to_int`/`cast_to_dec`'s missing Big support) — those are task 3e, a
  separate follow-up. (Note: Fix 3 above touches `compare.rs`'s Set/Map
  arms only, NOT its numeric Int↔Big arm — leave `compare_numeric`
  untouched.)
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, or the already-completed DDL-time-rejection /
  Call-in-tx / warn-log fixes (tasks 3a, 3b).
