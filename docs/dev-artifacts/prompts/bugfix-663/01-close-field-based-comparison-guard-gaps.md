# #663 — close the remaining gaps in the #651/#641 field-based-comparison guard

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — what #651 already fixed, and what it missed

`crates/shamir-query-types/src/batch/planner.rs`'s
`BatchPlanner::plan` rejects a `when` filter at PLAN time if it contains a
record-field-based comparison, via `contains_field_based_comparison`
(currently ~line 466). The reason: `when`'s `resolve_skip`
(`crates/shamir-engine/src/query/batch/query_runner.rs`) and write-value
marker resolution (`crates/shamir-engine/src/query/batch/param_subst.rs`'s
`resolve_write_value`) both evaluate a `Filter` against a **synthetic
record with no real fields** (`InnerValue::Null`, a scratch/dummy record) —
so a field-based comparison there does not error, it **silently folds to a
fixed boolean** (any `FieldPath` lookup against `Null` returns "absent",
which every comparison operator treats as non-matching). This is the exact
class of bug #651 fixed for `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq` inside
`when` (replacing the meaningful use case with `Filter::ValueCompare`,
which is genuinely record-free).

Two gaps remain, found during the final-session `@fh` review (F3/F4) and
independently confirmed by the orchestrator by reading the code:

### Gap 1 — the field-based variant list is incomplete

`Filter` (`crates/shamir-query-types/src/filter/filter_enum.rs`) has MORE
field-based variants than the 7 `contains_field_based_comparison` checks.
Every one of the following carries a `field: FieldPath` and resolves
against a real record — meaningless against `when`'s synthetic Null
record, exactly like `Eq`/`Ne`/etc:

```
Like, ILike, Regex, In, NotIn, Contains, ContainsAny, ContainsAll,
Between, Fts, VectorSimilarity, Computed
```

(`IsNull`/`IsNotNull`/`Exists`/`NotExists` also carry `field`, but stay
correctly EXCLUDED — they're a legitimate "does this key exist on the
record" presence guard against the synthetic record, already documented as
intentional in `contains_field_based_comparison`'s doc comment. Do not
touch that exclusion.)

Today, a `when` like `{"op": "like", "field": "status", "pattern": "%x%"}`
sails through `contains_field_based_comparison` undetected and silently
folds to a fixed `false` at runtime — the exact silent-fold bug #651 was
supposed to close, just via a variant the guard's list forgot.

### Gap 2 — the guard never looks inside `FilterValue` operands (nested `$cond`)

`contains_field_based_comparison` only walks the **structural** `Filter`
tree (`And`/`Or`/`Not` recursion). It never inspects the `FilterValue`
operands every comparison variant carries (`value`/`values`/`from`/`to`/
`expr_args`/`left`/`right`). But `FilterValue::Cond` embeds its OWN
`condition: Box<Filter>` (`crates/shamir-query-types/src/filter/cond.rs`) —
so a `when` like:

```
Filter::ValueCompare {
    left: FilterValue::Cond {
        cond: Cond { condition: Filter::Like { field: [...], pattern: ... }, then: ..., or_else: ... }
    },
    cmp: ValueCompareOp::Eq,
    right: FilterValue::Int(1),
}
```

passes today's check completely undetected (`ValueCompare` itself is
correctly NOT flagged — it has no field — but nobody ever looks at what's
INSIDE its `left`/`right` `FilterValue`s), and the nested `Cond`'s
`Like`-based condition silently folds exactly like Gap 1, just one level
deeper. This applies to EVERY `FilterValue` operand slot on every `Filter`
variant, not just `ValueCompare` — e.g. `Filter::In { values: [FilterValue::Cond{...}, ...], .. }`
has the identical gap.

### Gap 3 — write-value `$cond` markers (#641's class) have NO plan-time guard at all

`resolve_write_value` (`param_subst.rs`) resolves `$cond` markers inside
`InsertOp.values`/`UpdateOp.set`/`SetOp.{key,value}` against the SAME kind
of record-less dummy (`DUMMY_RECORD: InnerValue = InnerValue::Null`,
`param_subst.rs:105`) — identical silent-fold exposure to `when`. But
`BatchPlanner::plan`'s dependency-extraction pass over write values
(`extract_deps_from_value`, ~planner.rs:343) ONLY extracts `$query`
dependency edges from decoded markers — it has **zero** field-based-
comparison validation, unlike the `when` path. A write like:

```json
{"user_id": {"$cond": {"if": {"op": "like", "field": "status", "pattern": "%x%"}, "then": 1, "else": 0}}}
```

silently resolves `status` against `Null` (always "absent" → condition
always false → the field is always written as `0`) instead of erroring at
plan time, with no signal to the caller that their filter is meaningless
in this context.

## The fix

All three gaps share the same underlying primitive — extend and reuse it
consistently rather than writing three separate checks.

1. **Extend `contains_field_based_comparison`'s variant list** (Gap 1): add
   `Like`, `ILike`, `Regex`, `In`, `NotIn`, `Contains`, `ContainsAny`,
   `ContainsAll`, `Between`, `Fts`, `VectorSimilarity`, `Computed` to the
   `true` arm (alongside the existing `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/
   `FieldEq`). Leave `IsNull`/`IsNotNull`/`Exists`/`NotExists` excluded —
   do not change that documented exclusion. `ValueCompare` stays excluded
   at its OWN level (fixed by point 2 below, which looks inside it instead).

2. **Make the check recurse into `FilterValue` operands, to find nested
   `$cond` conditions** (Gap 2): write a companion function (suggested
   name `filter_value_contains_field_based_comparison(fv: &FilterValue) ->
   bool`) that returns `true` iff:
   - `fv` is `FilterValue::Cond { cond }` and EITHER
     `contains_field_based_comparison(&cond.condition)` is `true`, OR
     recursing this same function into `cond.then`/`cond.or_else` finds a
     nested field-based `$cond` (a `$cond`'s branches can themselves be
     `$cond`s).
   - `fv` is `FilterValue::Expr { expr }` and any of `expr.args` trips this
     function.
   - `fv` is `FilterValue::FnCall { call }` and any of `call.args()` trips
     this function.
   - `fv` is `FilterValue::Array(items)` and any item trips this function.
   - Otherwise `false` (in particular: `QueryRef`/`Param`/`FieldRef`/
     literals are never themselves field-based-comparison issues — a
     `$query`/`$param` reference is not a record-field lookup).

   Then update `contains_field_based_comparison` so that, for EVERY
   `Filter` variant that carries one or more `FilterValue` operands
   (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq`'s `value`; `In`/`NotIn`/
   `ContainsAny`/`ContainsAll`'s `values`; `Contains`'s `value`;
   `Between`'s `from`/`to`; `ValueCompare`'s `left`/`right`; `Computed`'s
   `value` and `expr_args`), it ALSO checks each of those operands via
   `filter_value_contains_field_based_comparison`, in addition to whatever
   its own top-level check already contributes. (A field-based variant
   like `Eq` is already `true` regardless — but still check its `value`
   operand too, since a currently-passing top-level variant like
   `ValueCompare` needs its `left`/`right` checked, and consistency here
   matters more than a handful of redundant `true`-anyway checks on the
   already-flagged variants.) `Like`/`ILike`/`Regex`/`Fts`/
   `VectorSimilarity` carry no `FilterValue` operand (their payload is a
   plain `String`/`Vec<f32>`) — nothing extra to check there beyond their
   own (already-`true`) variant check.

3. **Wire the guard into write-value `$cond` markers** (Gap 3):
   `extract_deps_from_value` (planner.rs, ~line 343) currently has
   signature `fn extract_deps_from_value(value: &QueryValue, deps: &mut
   TSet<String>)` with no error path. Change it to return
   `Result<(), BatchError>`, and thread an `alias: &str` parameter through
   for error attribution (mirroring how the `when`-guard call site already
   has `alias` in scope). Where it decodes a marker map into a
   `FilterValue` (the `is_query_fn_cond_expr_marker` branch) and the
   decoded value is `FilterValue::Cond { cond }`, call
   `contains_field_based_comparison(&cond.condition)` (point 1/2's now-
   extended check) — if `true`, return a new error (see point 4) instead of
   proceeding. Recurse normally otherwise (through `cond.then`/
   `cond.or_else` too, in case the guard-worthy `$cond` is nested deeper
   inside a branch value — reuse `filter_value_contains_field_based_comparison`
   here for the branches). Update every call site of
   `extract_deps_from_value` (`Update.set`, `Set.key`/`Set.value`, the
   `Insert.values` loop in `extract_dependencies`) to propagate the
   `Result` with `?`, and update `extract_dependencies`'s own signature
   (currently `fn extract_dependencies(op: &BatchOp) -> TSet<String>`) to
   `Result<TSet<String>, BatchError>`, taking `alias: &str` too. Update
   `plan()`'s call site (`let mut data_flow_deps =
   Self::extract_dependencies(&entry.op);`, ~line 164) to pass `alias` and
   propagate via `?`.

4. **New `BatchError` variant** (`crates/shamir-query-types/src/batch/batch_error.rs`):
   add `InvalidCondCondition { alias: String, message: String }` (mirrors
   `InvalidWhenFilter`'s shape exactly), plus its `Display` arm (mirror
   `InvalidWhenFilter`'s wording, e.g. `"invalid '$cond' condition in write
   value on '{alias}': {message}"`). Use the SAME message text style
   `InvalidWhenFilter` already uses ("field-based comparisons are not
   meaningful inside `$cond`'s condition when used in a write value (no
   record exists) — use Filter::ValueCompare for value-vs-value
   comparisons instead", adjusted wording as fits).

## Tests

Add to the existing `#651` test module in
`crates/shamir-query-types/src/batch/tests/` (find the file that already
tests `contains_field_based_comparison`/`InvalidWhenFilter` — check
`planner_tests.rs` or similar; follow its existing structure) plus a
`shamir-engine`-level integration test if the `when` guard's rejection is
also exercised there:

1. **Gap 1 regression, one case per newly-covered variant**: a `when`
   filter using each of `Like`/`ILike`/`Regex`/`In`/`NotIn`/`Contains`/
   `ContainsAny`/`ContainsAll`/`Between`/`Fts`/`VectorSimilarity`/
   `Computed` must be rejected by `BatchPlanner::plan` with
   `BatchError::InvalidWhenFilter`. (A single parametrized/table-driven
   test covering all 12 is fine — don't need 12 separate `#[test]`
   functions if a loop keeps it readable.)
2. **Gap 1 regression, legitimate exclusions still pass**: `when` using
   `IsNull`/`IsNotNull`/`Exists`/`NotExists` must still be ACCEPTED
   (`BatchPlanner::plan` returns `Ok`) — these must not regress into false
   positives.
3. **Gap 2**: a `when` filter shaped as `Filter::ValueCompare { left:
   FilterValue::Cond { cond: Cond { condition: Filter::Like { .. }, .. } },
   .. }` must be rejected with `InvalidWhenFilter` — the nested-inside-a-
   FilterValue-operand case. Also cover at least one more operand slot
   (e.g. `Filter::In { values: [FilterValue::Cond{...}], .. }`) to prove
   the check isn't special-cased to `ValueCompare` alone.
4. **Gap 2, positive/regression**: a `when` filter with a LEGITIMATE nested
   `$cond` whose condition IS `Filter::ValueCompare` (record-free) must
   still be ACCEPTED.
5. **Gap 3**: an `Insert`/`Update`/`Set` write value containing a `$cond`
   marker whose `if` is field-based (e.g. `Like`) must be rejected by
   `BatchPlanner::plan` with the new `BatchError::InvalidCondCondition` —
   at PLAN time, before any execution. Cover at least Insert; Update/Set
   are a bonus if time allows but Insert alone is the minimum bar.
6. **Gap 3, positive/regression**: an `Insert` write value containing a
   `$cond` marker whose `if` is `Filter::ValueCompare` (record-free, e.g.
   comparing a `$param` against a literal) must still PASS plan-time
   validation and resolve correctly at execution time (round-trip through
   `execute_batch_impl` or the existing engine-level write-value test
   harness — check `crates/shamir-engine/src/query/batch/tests/` for
   where `$cond`-in-write-value is already tested from #641 and add
   alongside).
7. **No regressions**: re-run the existing #651 tests
   (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq` rejection cases) — they must
   still pass unchanged.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full` green,
  including all new tests above.
- `cargo fmt -p shamir-query-types -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.

## Out of scope

- Do NOT touch #665, #666, #667 — separate tasks.
- Do NOT change `Filter::ValueCompare`'s own semantics, or `resolve_skip`/
  `resolve_write_value`'s execution-time resolution logic — this task is
  entirely about the PLAN-TIME static guard, not runtime behavior.
- Do NOT change the `IsNull`/`IsNotNull`/`Exists`/`NotExists` exclusion —
  it is intentional and documented, not a gap.
- Do NOT add a wire/serde-level change to `Filter`/`FilterValue`/`Cond` —
  this is a validation-only change inside `shamir-query-types`'s planner
  plus a matching engine-level test, no schema changes.
