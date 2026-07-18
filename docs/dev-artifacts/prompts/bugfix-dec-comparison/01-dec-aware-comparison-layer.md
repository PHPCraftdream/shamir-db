# Bug — Dec/Big blind spot in the comparison layer breaks filters, aggregates, and ORDER BY

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## Background — why this matters

`shamir-funclib` is deliberately "decimal-first": every math scalar
(`abs`, `round`, `pow`, `sqrt`, `mod`, ...) returns `QueryValue::Dec`
(`crates/shamir-funclib/src/registry.rs:306-315`), and the write path
persists Dec results into records for computed fields
(`crates/shamir-engine/src/table/write_helpers.rs:132-135`). But the
engine's comparison layer never learned about `Dec` (or `Big`), so a
Dec-valued column or a Dec-valued `$fn` result silently breaks numeric
filtering, aggregation, and sorting — with NO error, just wrong answers.
This task closes that gap in three places. **Fix all three; they share
one root cause but need separate, precisely-scoped changes.**

## Part 1 — filters: `$fn` Dec results never match in WHERE / `$expr`

### 1a. `scalar_ref_cmp_qv` / `scalar_ref_cmp` — cross-type Dec/Big arms

`crates/shamir-types/src/record_view/scalar_ref.rs:78-109`. `ScalarRef`
itself (lines 35-48) deliberately has NO `Dec`/`Big` variant — **do NOT
add one; that is out of scope and would ripple through every `ScalarRef`
consumer in the workspace (index extraction, other Stage-3 consumers)**.
The actual bug this task targets is narrower and safe to fix: when the
**record FIELD** is a normal `Int`/`F64` (so `record.scalar_at` correctly
returns `ScalarRef::Int`/`ScalarRef::F64`) but the **filter operand** —
typically a `{"$fn": ...}` result already resolved to a `QueryValue` — is
`Dec` or `Big`, `scalar_ref_cmp_qv(ScalarRef::Int(_), &QueryValue::Dec(_))`
falls to `_ => None` (no matching arm), and `Compare::matches`
(`crates/shamir-engine/src/query/filter/filter_node.rs:305-323`) treats a
`None` comparison as `false` for `Eq/Gt/Gte/Lt/Lte` and `true` for `Ne` —
silently, for every row.

**Trigger:** `{"op": "gt", "field": "price", "value": {"$fn": {"name":
"abs", "args": [-100]}}}` where `price` is a normal Int field. `abs(-100)`
resolves to `QueryValue::Dec(100)`. Today this matches NO rows regardless
of `price`'s actual value; with `"op": "ne"` it matches ALL rows.

**Fix:** add cross-type arms to `scalar_ref_cmp_qv` (and its `InnerValue`
twin `scalar_ref_cmp`, keep them in sync — the module doc says they must
mirror arm-for-arm):
- `(ScalarRef::Int(a), QueryValue::Dec(b))` → compare via
  `rust_decimal::Decimal::from(a).partial_cmp(b)` (exact — `Decimal` can
  represent every `i64` exactly) or equivalent.
- `(ScalarRef::F64(a), QueryValue::Dec(b))` → convert `b` to `f64` via
  `Decimal::to_f64()` (or the crate's existing Dec→f64 helper if one
  exists — search for `to_f64`/`as_f64` usages on `Decimal` elsewhere in
  the codebase first) and `partial_cmp`, matching the existing
  Int↔F64 cross-type pattern's style (f64 fallback, not exact — this
  mirrors the report's own accepted tradeoff).
- Add the symmetric arms with operands swapped
  (`(ScalarRef::Dec-side-is-b, ...)` doesn't apply since `ScalarRef` has no
  Dec variant — but you DO need `QueryValue::Dec` on either side relative
  to `Int`/`F64`, i.e. both `(Int, Dec)`-shape AND check whether the
  compile actually needs a reversed arm given `scalar_ref_cmp_qv`'s
  signature is `(ScalarRef, &QueryValue)` — the record field is always
  the `ScalarRef` side and the filter operand is always the `QueryValue`
  side, so only ONE direction is structurally possible here; confirm this
  by reading the call site before assuming you need a mirrored arm).
- `Big` (i64::MAX-scale integers from arithmetic overflow): add the same
  shape of arm via an f64 fallback (`BigInt::to_f64` — accept the
  precision-loss tradeoff for large values; this is EXPLICITLY tracked as
  a separate, lower-priority finding elsewhere — do not attempt to fix
  Big's precision loss in this task, just stop it from being a silent
  `None`/no-match).

### 1b. `compare_values` — Dec/Big arms (feeds Min/Max's container fallback and any direct `Value<K>` comparison)

`crates/shamir-engine/src/query/filter/resolve.rs:91-105`. Currently only
Null/Bool/Int/F64/Str have arms; everything else (including Dec vs Dec,
Dec vs Int, Dec vs F64, Big vs anything) falls to `_ => None`. Add:
- `(Value::Dec(a), Value::Dec(b))` → `Some(a.cmp(b))` (exact, `Decimal`
  implements `Ord`).
- `(Value::Int(a), Value::Dec(b))` and `(Value::Dec(a), Value::Int(b))` →
  exact via `Decimal::from(i64)` comparison.
- `(Value::F64(a), Value::Dec(b))` and `(Value::Dec(a), Value::F64(b))` →
  f64 fallback (convert Dec to f64, `partial_cmp`).
- `Big` arms: same shape, f64 fallback, same precision-loss caveat as 1a
  (out of scope to fix precision here).
- Read the existing doc comment above `compare_values` (lines 80-90,
  the #667 3-way null-semantics note) — do not disturb the `(Null, Null)`
  arm or the overall `None`-means-"nothing to compare" contract; you are
  ONLY adding new matched arms, not changing existing semantics.

### 1c. `$expr` numeric coercion accepts Dec

`crates/shamir-engine/src/query/filter/resolve.rs:290-297` — the local
`as_f64` helper inside `eval_filter_expr` only accepts `Int`/`F64`. A Dec
operand (e.g. `{"$expr": {"op": "add", "args": [{"$fn": {...}}, 1]}}`
where the `$fn` result is Dec) collapses to `None`, silently making the
enclosing comparison false (or the whole expr absent). Add a `Dec` arm to
`as_f64` (convert via f64, same fallback as above). Leave `as_str`/
`as_bool` untouched — not part of this bug.

## Part 2 — aggregates: Sum/Avg/Min/Max silently wrong over Dec columns

`crates/shamir-engine/src/query/read/aggregate.rs`. Read `AggAccum`/
`AggState`/`step`/`finish` in full (roughly lines 294-560) before editing
— Min/Max ALREADY have a "container/Dec/Big leaf" fallback branch (the
`else` arm in `step`'s `Min`/`Max` cases, around lines 435-467 for Min and
483-506 for Max) that calls `record.materialize_at(path)` when
`scalar_at` returns `None` (which it does for a Dec/Big field, since
`ScalarRef` has no such variant) and compares via `compare_values` on the
owned `Value` tree. **Once you've added the Dec arms to `compare_values`
in Part 1b, this Min/Max fallback should start working correctly for Dec
columns with NO further change needed to Min/Max itself** — verify this
is actually true by testing it (see Tests section) rather than assuming.

**Sum and Avg have NO equivalent fallback at all** — today, when `scalar`
is `None` (i.e. the field is Dec/Big/container), `AggState::Sum`'s `step`
just does nothing (`if let Some(s) = scalar { ... }` — the `None` case is
implicit and silently skips the row), same for `Avg`. This is the actual
bug: sum/avg over a Dec column silently produces `Int(0)`/`Null` instead
of the correct total.

**Fix:** add a container/Dec fallback branch to `AggState::Sum`'s and
`AggState::Avg`'s `step` arms, mirroring the pattern Min/Max already use
(materialize the field via `record.materialize_at(path)` when `scalar_at`
returned `None`) — but note Sum/Avg need the VALUE, not just a comparison,
so: on the fallback path, materialize the field, and if it resolves to
`InnerValue::Dec(d)` or `InnerValue::Big(b)`, convert to `f64` (same
fallback tradeoff as elsewhere in this task) and add it to the existing
`sum_f`/`has_float` (Sum) or `sum`/`count` (Avg) accumulators — i.e. Dec
values flow into the SAME f64 accumulator lane that F64 values already
use, just reached via the materialize fallback instead of the direct
`ScalarRef` path. Container leaves (Map/List/Set) still contribute
nothing (unchanged — those were never numeric).

Compare this against the funclib `AggregateFn` path's own
`fn_value_for_aggregator` (`aggregate.rs:804-825`), which the report notes
ALREADY handles Dec correctly for the exact same data via this same
materialize-fallback shape — use it as your working reference for how the
fallback should look structurally (it is not a byte-for-byte template
since it returns a `QueryValue` for the funclib boundary rather than
accumulating in place, but the "try scalar_at first, materialize_at as
fallback" shape is the one to mirror).

## Part 3 — ORDER BY: Dec/Big sorts lexicographically instead of numerically

`crates/shamir-engine/src/query/read/order.rs`. `QvSortKey`
(`enum`, lines ~158-165) currently has no numeric Dec variant;
`from_query_value` (lines 174-187) maps `Dec(d) → Str(d.to_string())` and
`Big(b) → Str(b.to_string())`, and `compare_qv_sort_keys`'s `base` match
(lines 266-280) then compares those as plain strings — so `[9.5, 10.5, 2]`
sorts as `["10.5", "2", "9.5"]` (lexicographic), not `[2, 9.5, 10.5]`
(numeric).

**Fix:**
1. Add a `Dec(rust_decimal::Decimal)` variant to `QvSortKey`.
2. In `from_query_value`, map `QueryValue::Dec(d) => QvSortKey::Dec(*d)`
   (or `d.clone()`/copy as appropriate for `Decimal`'s `Copy`-ness — check
   whether `Decimal` implements `Copy` before deciding `*d` vs cloning).
3. In `compare_qv_sort_keys`'s `base` match, add:
   - `(QvSortKey::Dec(x), QvSortKey::Dec(y)) => x.cmp(y)` (exact,
     `Decimal: Ord`).
   - `(QvSortKey::I64(x), QvSortKey::Dec(y))` and
     `(QvSortKey::Dec(x), QvSortKey::I64(y))` → exact via
     `Decimal::from(i64)` comparison, matching the existing
     I64↔F64 cross-type arm's style.
   - `(QvSortKey::F64(x), QvSortKey::Dec(y))` and
     `(QvSortKey::Dec(x), QvSortKey::F64(y))` → f64 fallback (convert Dec
     to f64), matching the existing F64↔I64 arm's `partial_cmp(...)
     .unwrap_or(Equal)` style.
4. Leave `Big` mapped to `Str(b.to_string())` as-is — **out of scope for
   this task** (BigInt sort keys are a separate, lower-priority item; the
   report groups Dec as the primary, high-frequency fix and Big as a
   secondary concern). Do not attempt a numeric `Big` sort key here unless
   it is trivial to add alongside the `Dec` variant with no extra
   complexity — use your judgement, but Dec is the required deliverable.

## Tests

Add tests to the existing test modules for each area (find the exact file
per area — likely `crates/shamir-engine/src/query/filter/tests/`,
`crates/shamir-engine/src/query/read/tests/` or wherever aggregate/order
tests currently live; follow existing conventions for record/filter
construction):

1. **Filter WHERE against a Dec `$fn` result**: `WHERE price > {"$fn":
   {"name": "abs", "args": [-100]}}` (or equivalent construction through
   whatever the codebase's test helpers use to build such a filter) over
   rows with `price` values both above and below 100 — must correctly
   include/exclude rows numerically, not silently match nothing.
2. **`$expr` arithmetic over a Dec operand**: an `$expr` `add`/`gt` (or
   similar) with one operand resolving to Dec — must produce the correct
   numeric result, not silently-absent.
3. **`sum`/`avg` over a Dec column**: seed rows with known Dec values,
   assert the aggregate returns the correct numeric total/average, not
   `Int(0)`/`Null`.
4. **`min`/`max` over a Dec column**: seed rows with Dec values in a
   non-monotonic insertion order, assert the true extreme is returned
   (not just whichever row was scanned first — this is the regression
   test proving the `compare_values` fix actually unblocks the existing
   Min/Max container fallback).
5. **ORDER BY over a Dec column**: seed rows with Dec values
   `[9.5, 10.5, 2]` (or equivalent), assert ascending ORDER BY returns
   `[2, 9.5, 10.5]` (numeric order), not lexicographic string order.
6. **Cross-type ORDER BY**: a mix of Dec and Int/F64 values in the same
   sort key position — assert they compare numerically against each
   other (not falling to the `_ => Equal` arbitrary-order arm).
7. **Regression**: existing filter/aggregate/ORDER BY tests over
   Int/F64/Str/Bool columns must continue to pass completely unchanged —
   this fix only ADDS new matched arms, never changes existing behavior
   for non-Dec/Big types.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-engine`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the Min/Max container fallback DOES now
  correctly compare Dec-vs-Dec once `compare_values` has the new arms (a
  test proving this, not just an assertion), (b) Sum/Avg have a genuinely
  NEW fallback branch (they didn't have any before) rather than an
  assumption that the existing `scalar_at` path would somehow start
  working, (c) no existing non-Dec/Big test's behavior changed.

## Out of scope

- Do NOT add a `Dec`/`Big` variant to `ScalarRef` itself — that is a
  larger, separate architectural change rippling through every
  `ScalarRef` consumer in the workspace (index extraction and other
  Stage-3 consumers), not needed for this bug (the field-side extraction
  problem is solved via the existing `materialize_at` container fallback
  pattern, not by extending `ScalarRef`).
- Do NOT fix `Big`'s precision-loss-via-f64 comparison issue (a separate,
  already-tracked, lower-priority finding) beyond making it not silently
  `None`/lexicographic — an f64-fallback comparison for Big is acceptable
  here; do not attempt exact BigInt arithmetic comparison.
- Do NOT touch `$contains_all`, FK on-update/cascade, or UPSERT
  `created_at` — those are separate, already-fixed or separately-tracked
  tasks.
- Do NOT touch `count_distinct`/`mode` over Set/Map (a separate,
  already-known, lower-priority finding about container equality).
