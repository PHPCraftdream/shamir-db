# Funclib top-up 4b — wire params for percentile/string_agg + honor-or-reject `distinct`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Second P0 item of "Этап 4 — v0.10 funclib top-up"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 10 (`docs/dev-artifacts/research/2026-07-17-release-audit/10-release-readiness-v0.10.md`,
~lines 40-43, 141-142, 154):

> Parameterised aggregates are unreachable from the wire — `percentile` is
> hard-wired to p=0.5 and `string_agg` to sep="," because
> `SelectItem::AggregateFn { name, field, alias, distinct }` has no params
> field and the executor calls `builtin_aggs().make(name)` with no params.
> The `distinct: bool` flag on `Aggregate`/`AggregateFn` is deserialized
> but ignored by the grouped executor — `sum`/`avg`/`string_agg` DISTINCT
> silently degrade to non-distinct. Should be honored or rejected, never
> ignored.

This brief covers TWO related but independent fixes. Both are scoped from
direct reading of the current code (line numbers accurate as of this
brief; re-verify before editing, this campaign has touched
`crates/shamir-engine/src/query/read/aggregate.rs` in earlier stages).

---

## Fix 1 — wire-level params for `percentile`/`string_agg`

### Current state

- `crates/shamir-query-types/src/read/select.rs`'s `SelectItem::AggregateFn`
  (~lines 85-93) has `name`, `field`, `alias`, `distinct` — no `args`
  field. Compare to the sibling `SelectItem::Function` variant (~lines
  99-106) which already has `args: Vec<FilterValue>` for scalar
  projections — mirror that shape.
- `crates/shamir-funclib/src/agg.rs` already has fully-working
  PARAMETERIZED factory functions: `pub fn percentile(p: f64) -> AggFactory`
  (~line 478) and `pub fn string_agg(sep: String) -> AggFactory` (~line
  580). `register()` (~lines 98-122) only wires the DEFAULT-parameter
  versions into the name-keyed `AggRegistry` (`percentile` → p=0.5,
  `string_agg` → sep=","). The parameterized factories are public and
  ready to use — they are simply never called with anything other than
  the hard-coded defaults today.
- `crates/shamir-engine/src/query/read/aggregate.rs`'s
  `build_aggregate_object` (~line 704-718, inside the `SelectItem::AggregateFn`
  match arm) does `fn_slots.push((key, field_path, all_field,
  builtin_aggs().make(name)))` — always the no-args registry lookup,
  never the parameterized factory.

### The fix

1. Add `#[serde(default)] args: Vec<FilterValue>` to `SelectItem::AggregateFn`
   in `crates/shamir-query-types/src/read/select.rs`, mirroring
   `SelectItem::Function`'s `args` field exactly (same serde attributes).
   Check `crates/shamir-query-builder`'s corresponding builder API (search
   for wherever `AggregateFn`/`agg_fn`-shaped builder methods live — likely
   near the `Select`/aggregate builder helpers) and add an `args`-accepting
   variant there too, per this project's **builder-only discipline** (CLAUDE.md:
   never hand-assemble query shapes with raw JSON — the query builder is
   the only sanctioned construction path). Also check
   `crates/shamir-client-ts`'s TypeScript builder for the equivalent
   `AggregateFn`/aggregate-with-args shape and add it there too if the TS
   builder currently exposes `percentile`/`string_agg` as select-projection
   helpers (grep for them) — keep both builders in sync. If the TS client
   doesn't currently expose these aggregates at all, adding TS support is
   OUT OF SCOPE for this brief (Rust-side wire capability first; TS parity
   is a separate concern only if it already partially exists).
2. In `aggregate.rs`'s `build_aggregate_object`, at the `SelectItem::AggregateFn`
   arm, when `name` is `"percentile"` or `"string_agg"` AND `args` is
   non-empty, extract the literal parameter directly (these are static
   SELECT-clause constants, not per-row values — do NOT thread a
   `FilterContext`/record resolution through this, just match the
   `FilterValue` variant directly: `FilterValue::Float(p)`/`FilterValue::Int(p)`
   for percentile's `p: f64` — accept both and convert Int to f64 — and
   `FilterValue::String(s)` for string_agg's `sep: String`). Call
   `shamir_funclib::agg::percentile(p)()` / `string_agg(sep)()` directly
   instead of `builtin_aggs().make(name)` when args are present and valid.
   If `name` is `"percentile"`/`"string_agg"` but the arg is a non-literal
   (`$ref`/`$fn`/`$expr`/`$cond`/`$param` — anything dynamic) or the wrong
   type, or if a name OTHER than these two receives non-empty `args`
   (no other funclib aggregate in this registry takes params today — check
   `agg.rs`'s `register()` to confirm this is still true when you read it),
   reject with a coded error (e.g. `agg_params_not_supported` or
   `agg_param_must_be_literal`, pick names consistent with this codebase's
   snake_case coded-error convention) rather than silently ignoring the
   bad/unsupported args. Empty `args` on `percentile`/`string_agg`
   continues to use the existing default-parameter registry lookup
   (backward compatible — no behavior change for existing callers).
3. Validate `percentile`'s `p` is in `[0.0, 1.0]` before constructing the
   aggregator (check what `PercentileAgg`/`percentile()` itself already
   validates, if anything, and don't duplicate a check that already
   exists downstream — but if there's no validation today, add one here
   with a coded error like `out_of_range`).

### Tests

1. `SELECT percentile(score, args: [0.9])` (however the query-builder
   surfaces this — follow its actual method signature once you've added
   it) returns the 90th percentile, not the median.
2. `SELECT string_agg(name, args: ["; "])` joins with `"; "`, not `","`.
3. Regression: `percentile`/`string_agg` with NO args still default to
   p=0.5/sep="," exactly as before.
4. `percentile` with `p` outside `[0, 1]` (e.g. `1.5`) is rejected with a
   coded error, not silently clamped or producing garbage.
5. A dynamic (non-literal) arg to `percentile`/`string_agg` (e.g. a
   `$ref` field reference) is rejected with a coded error, not silently
   ignored or defaulting.
6. Some OTHER aggregate name (e.g. `"median"`) receiving non-empty `args`
   is rejected with a coded error (no other funclib aggregate accepts
   params today).

---

## Fix 2 — honor-or-reject the `distinct` flag

### Current state

Two independent aggregation paths both silently ignore `distinct` today:

- `SelectItem::Aggregate { func, field, alias, .. }` (~line 683-696) — the
  CLOSED fast-path set (`Count`/`Sum`/`Avg`/`Min`/`Max`, dispatched via
  `AggAccum::new(*func, field, interner)`, a custom hand-rolled state
  machine, NOT going through the `Aggregator` trait). The `..` in the
  destructure explicitly discards `distinct`.
- `SelectItem::AggregateFn { name, field, alias, .. }` (~line 704-718) —
  the funclib-dispatched set (`median`, `mode`, `stddev`, `variance`,
  `percentile`, `string_agg`, `array_agg`, `bool_and`/`bool_or`, `first`/
  `last`, `range`, `count_distinct` — all going through
  `Box<dyn Aggregator>`). Same `..` discard.

These two paths need DIFFERENT treatment because of how differently they
are architected — do not try to force one uniform mechanism onto both.

### Fix 2a — `SelectItem::AggregateFn` path: HONOR `distinct` via a generic wrapper

`crates/shamir-funclib/src/agg.rs`'s `Aggregator` trait (~lines 45-53) is
minimal — `accumulate(&mut self, v: &QueryValue)` /
`finalize(self: Box<Self>)`. This makes a generic distinct-dedup wrapper
straightforward and low-risk to add:

```rust
/// Wraps any `Aggregator`, skipping values already seen (via
/// `crate::compare::compare`, the workspace cross-type total order — same
/// dedup strategy `CountDistinctAgg` already uses) before delegating to the
/// inner aggregator. `Null` values pass through to the inner aggregator
/// unchanged (each aggregator already decides its own Null handling).
struct DistinctWrapper {
    inner: Box<dyn Aggregator>,
    seen: Vec<QueryValue>,
}

impl Aggregator for DistinctWrapper {
    fn accumulate(&mut self, v: &QueryValue) -> Result<(), ScalarError> {
        if !is_null(v) {
            let already = self.seen.iter().any(|s| compare::compare(s, v) == Ordering::Equal);
            if already {
                return Ok(());
            }
            self.seen.push(v.clone());
        }
        self.inner.accumulate(v)
    }

    fn finalize(self: Box<Self>) -> Result<QueryValue, ScalarError> {
        self.inner.finalize()
    }
}
```

(Adjust field/method visibility and exact location — this belongs in
`agg.rs` near `CountDistinctAgg`, which already establishes the same
`compare`-based dedup pattern; read that implementation first and mirror
its style exactly, including whatever O(n²) tradeoff comment it carries,
since this shares the same complexity characteristics.) In
`aggregate.rs`'s `SelectItem::AggregateFn` arm, when `distinct` is `true`,
wrap the constructed aggregator: `if *distinct { Some(Box::new(DistinctWrapper
{ inner: agg, seen: Vec::new() })) } else { Some(agg) }` (adapt to
whatever the actual `Option<Box<dyn Aggregator>>` construction looks like
once you've made Fix 1's edits in the same arm — both fixes touch this
same match arm, do them together coherently rather than as two separate
passes over the same code).

**Note**: `count_distinct` already IS its own dedicated distinct aggregate
(the report notes this). If a query sets `distinct: true` on
`AggregateFn { name: "count_distinct", .. }`, wrapping it in `DistinctWrapper`
would double-dedup — harmless (idempotent) but pointless. Don't special-case
it away; the wrapper being a no-op-if-redundant is fine and simpler than
adding an exclusion list.

### Fix 2b — `SelectItem::Aggregate` (closed fast-path) — REJECT `distinct` for Sum/Avg/Count; allow (no-op) for Min/Max

`AggAccum`'s `Sum`/`Avg`/`Count` state machines (`AggState` enum, read the
whole enum and `step`/`finish` before editing) accumulate directly into
scalar fields (`sum_i`/`sum_f`, running count) with no per-value dedup
tracking — retrofitting real distinct support here means restructuring
this hot-path state machine to also carry a `seen: Vec<QueryValue>` (or
similar), which is a heavier, riskier change than this LOW/MED-priority
wire-parity fix calls for. Per this campaign's established precedent for
"architecturally heavier than this fix's scope" cases (see the
self-referential-CASCADE DDL-time-rejection decision from an earlier
stage of this same campaign, task 3d): **convert the silent-wrong
behavior into an honest, explicit rejection** for the cases where it
would actually change the result:

- `distinct: true` with `func` = `Sum` or `Avg` or `Count` → reject with a
  coded error (e.g. `distinct_not_supported_for_fast_path_agg`) at the
  point `AggAccum` would otherwise be constructed, naming the aggregate
  and suggesting the funclib-dispatched equivalent exists for `count`
  (`count_distinct` via `AggregateFn`) as an alternative — check whether
  `sum`/`avg` have funclib-dispatched equivalents too (they likely don't,
  since `sum`/`avg` are closed-fast-path-only per `AggFunc`'s definition —
  confirm this, and word the error message accordingly, don't claim an
  alternative that doesn't exist).
- `distinct: true` with `func` = `Min` or `Max` → **allow, silently
  no-op** (min/max of a value set is identical to min/max of its distinct
  subset — there is no silent-wrong-result risk here, `distinct` merely
  has no observable effect, which is a true and correct fact about
  min/max, not a bug to reject). Do NOT reject this case — only Sum/Avg/
  Count carry the actual silent-wrong-result risk the report is
  concerned about.

### Tests

1. `AggregateFn` distinct: `string_agg` (or any other funclib-dispatched
   aggregate) with `distinct: true` over duplicate values produces the
   deduplicated result (e.g. `string_agg` over `["a", "a", "b"]` with
   `distinct: true` and default sep `,` → `"a,b"`, not `"a,a,b"`).
2. Regression: same aggregate with `distinct: false` (or omitted) keeps
   duplicates exactly as before.
3. `Aggregate { func: Sum, distinct: true, .. }` is rejected with the new
   coded error, not silently computing a non-distinct sum.
4. Same for `Avg` and `Count`.
5. `Aggregate { func: Min, distinct: true, .. }` (and `Max`) succeeds
   normally (no rejection) — confirm this explicitly, don't just assume
   the reject-path covers Min/Max too by accident.
6. Regression: `distinct: false`/omitted on the fast-path set behaves
   exactly as before for all five functions.

## Out of scope

- Do NOT restructure `AggAccum`'s Sum/Avg state machine to support real
  distinct tracking — Fix 2b explicitly rejects those cases instead.
- Do NOT touch any OTHER Этап 4 P0 item (null functions — already done in
  task 4a; datetime format/parse, uuid_v4, arrays/sort, parse_json/to_json
  — separate later leaf tasks).
- Do NOT add TypeScript client (`shamir-client-ts`) support for these
  aggregates if it doesn't already partially exist (see Fix 1, step 1's
  caveat).
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, or task 4a — this brief is scoped to Этап 4's P0 item 2
  only.

## Verification (MANDATORY before you report done, for BOTH fixes)

- `./scripts/test.sh @engine --full` green (covers `aggregate.rs` and the
  query-builder wiring), including all new/modified tests.
- `./scripts/test.sh -p shamir-funclib --full` green (covers `agg.rs`'s
  `DistinctWrapper`).
- Confirm whether the `SelectItem`/query-builder changes need any other
  scope run (e.g. `-p shamir-query-types`, `-p shamir-query-builder`) and
  run those too if their own test suites exist and are affected.
- `cargo fmt --all -- --check` clean (or scoped to touched crates, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the query builder(s) you touched (Rust, and TS
  only if in-scope per Fix 1 step 1) expose the new `args`/parameterized
  aggregate shape — no raw-JSON construction anywhere in your new tests;
  (b) `DistinctWrapper` is a genuinely generic wrapper, not special-cased
  per aggregate name; (c) the fast-path rejection in Fix 2b applies to
  Sum/Avg/Count only, and Min/Max continue to accept `distinct: true` as
  a harmless no-op.
