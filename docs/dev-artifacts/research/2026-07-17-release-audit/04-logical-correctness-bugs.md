# Release audit 04 — Logical / correctness bugs

Date: 2026-07-17
Scope: business-logic correctness only (filter evaluation, aggregates, FK actions,
ORDER BY, batch executor, funclib). Security, permissions, and concurrency are
covered by other audit sheets. Every finding below was traced through the actual
code paths cited — file:line references are to the working tree at audit time.

---

## Executive summary

The single most damaging **theme** is a Dec/Big blind spot in the engine's
comparison layer: funclib is deliberately "decimal-first" (every math scalar
returns `QueryValue::Dec` — `registry.rs:306-315`), and the write path persists
those Dec results into records (computed fields, `write_helpers.rs:132-135`),
but `compare_values` (`resolve.rs:91-105`), `scalar_ref_cmp_qv`
(`scalar_ref.rs:98-109`), the lens `ScalarRef` (no Dec variant), the built-in
aggregate accumulators (`aggregate.rs`) and the ORDER BY key extractor
(`order.rs:174-187`) all either ignore Dec or degrade it. The compounded result:
a Dec-valued column **cannot be filtered numerically, sums to 0, min/max returns
an arbitrary row, and sorts lexicographically** — all silently, with no error.

Independent of that theme, three high-severity discrete bugs were found:
`$contains_all`'s set fast-path counts duplicate field elements toward the
required total (wrong `true`); the ON UPDATE FK planner dedups away every FK
reference after the first per parent field (silently unenforced referential
actions); and the ON DELETE cascade planner's cycle guard rejects legal diamond
FK topologies with a spurious `fk_cascade_depth` error.

| # | Severity | Area | Finding |
|---|----------|------|---------|
| 1 | HIGH | filter eval | `$contains_all` fast-path counts duplicates — matches when required values are absent |
| 2 | HIGH | FK on-update | `dedup_by(parent_ref_field)` drops all but one FK ref — restrict/cascade/setnull silently skipped |
| 3 | HIGH | FK cascade | Diamond (multi-path DAG) cascade falsely rejected as "cycle detected" |
| 4 | HIGH | aggregates | Built-in sum/avg/min/max silently wrong over Dec/Big columns (0 / Null / first-row value) |
| 5 | HIGH | filter eval | Filters comparing against `$fn` math results (Dec) never match; `$expr` arithmetic over Dec collapses to absent |
| 6 | HIGH | ORDER BY | Dec/Big sort keys are `to_string()` — lexicographic order ("10.5" < "9.5") |
| 7 | MED | filter eval | `$in`/`$contains_any`/`$contains_all` all-literal fast paths drop Int↔F64 coercion the slow paths have |
| 8 | MED | query exec | User-registered scalars silently unavailable in SELECT projections, group SELECT fns, `when`, `bind`, `over` |
| 9 | MED | aggregates | `count_distinct`/`mode` over Set/Map values use length-only equality — distinct maps counted as one |
| 10 | MED | FK actions | Self-referential FKs silently unenforced (discovery skips the parent table) |
| 11 | MED | FK actions | FK child matching is strict-typed (Int(1) ≠ F64(1.0)) — cascade/setnull/restrict can miss rows |
| 12 | LOW | filter eval | `$expr` `mod` uses unchecked `%` — `i64::MIN % -1` panics in debug builds |
| 13 | LOW | aggregates / datetime | Unchecked i64 arithmetic: engine Sum accumulator, `diff_secs` |
| 14 | LOW | funclib compare | Int↔Big comparison via f64 — distinct large values compare Equal |
| 15 | LOW | funclib cast | `to_int`/`to_dec`/`to_float` reject `Big` even when it fits |

---

## Detailed findings

### 1. HIGH — `$contains_all` set fast-path counts duplicate field elements

**Where:**
- `crates/shamir-engine/src/query/filter/filter_node.rs:601-624` (`FilterNode::ContainsAllSet::matches`)
- `crates/shamir-engine/src/query/filter/compile.rs:286-305` (`compile_contains_all_node` — this fast path is chosen whenever ALL filter values are literals, i.e. the common case)

**What I read/traced:** `ContainsAllSet` counts how many field-array elements are
members of the literal set, then passes when `found >= required` where
`required = values.len()`:

```rust
let required = values.len();
let found = match &field_qv {
    QueryValue::List(list) => list.iter().filter(|item| values.contains(*item)).count(),
    ...
};
found >= required
```

A `QueryValue::List` field may contain duplicates. Each duplicate that is a
member of the set increments `found` — so duplicates of ONE required value can
stand in for the OTHER required values.

**Trigger:** filter `{"tags": {"$contains_all": ["a", "b"]}}` against a record
with `tags = ["a", "a"]`. `required = 2`, `found = 2` (both `"a"`s hit) →
**matches**, although `"b"` is absent. The slow-path twin `ContainsAll`
(filter_node.rs:573-599, taken only when some value is non-literal) correctly
evaluates `values.iter().all(|fv| field contains fv)` and returns `false` for
the same input — so the same logical filter gives different answers depending
on whether its list happens to be all-literal.

**Correct behavior:** count *distinct set members found* (e.g. track which set
members were seen, or check `values.iter().all(...)` against a field-element
set), not raw element hits.

---

### 2. HIGH — ON UPDATE FK planner dedups away all but one FK reference per parent field

**Where:** `crates/shamir-engine/src/query/batch/fk_on_update.rs:179-180`, used
by steps 4–5 at lines 204-238.

**What I read/traced:**

```rust
relevant_refs.sort_unstable_by(|a, b| a.parent_ref_field.cmp(&b.parent_ref_field));
relevant_refs.dedup_by(|a, b| a.parent_ref_field == b.parent_ref_field);
```

The dedup key is **only** `parent_ref_field`. But `relevant_refs` — the deduped
vec — is subsequently used to build `ref_fields` (fine), **and** the
`by_table` grouping and per-child probes (lines 231-238, 244-267), which is not
fine: two different FK references sharing a parent field are collapsed to one.
The comment `// already deduped above` at line 207 shows the dedup was intended
for the *field list*, not the ref list. Compare the delete path
(`fk_actions.rs:168-176`), which correctly dedups only the derived
`parent_ref_fields` vector and keeps `action_refs` whole.

**Trigger (two child tables):** `users.id` referenced by
`orders.user_id (ON UPDATE CASCADE)` and `sessions.user_id (ON UPDATE CASCADE)`.
Both refs have `parent_ref_field == "id"`; after sort+dedup only one survives.
Updating a user's `id` re-keys `orders` but leaves `sessions.user_id` pointing
at the old value — **silent dangling references**, no error.

**Trigger (one child table, two FKs):** `messages.sender_id` and
`messages.receiver_id` both → `users.id`. Only one field is cascaded.

**Trigger (RESTRICT variant):** if the dropped ref is the RESTRICT one, the
parent update is **allowed** even though children still reference the old
value — the declared referential action is silently not enforced.

**Correct behavior:** dedup only the field-name list used for
`collect_parent_values`/`new_values`; keep every `OnUpdateRef` for probe
construction.

---

### 3. HIGH — cascade planner rejects legal diamond FK topologies as cycles

**Where:** `crates/shamir-engine/src/query/batch/fk_actions.rs:148-159`
(`plan_cascade_recursive` cycle guard) and `:380-391` (`plan_cascade_for_ids`
cycle guard); the shared `visited` set is created once in `plan_cascade`
(`:98-115`) and entries are **never removed** when a recursion branch returns.

**What I read/traced:** `visited` is a `TFxSet<String>` of table names threaded
by `&mut` through the whole plan. On re-encounter the planner returns
`Err(fk_cascade_depth, "cascade cycle detected at table '...'")` — aborting the
entire DELETE. Because entries persist across *sibling* branches, any table
reachable via two distinct FK paths trips the guard even though the FK graph is
an acyclic DAG.

**Trigger:** tables `B` and `C` both have `ON DELETE CASCADE` FKs to `A`;
table `D` has cascade FKs to both `B` and `C` (a diamond). `DELETE FROM A ...`
where the delete cascades rows through both branches:
`plan_cascade_recursive(A)` → branch B → `plan_cascade_for_ids(D)` inserts
`"D"`; then branch C → `plan_cascade_for_ids(D)` → `visited.insert("D")` fails
→ the whole delete errors with `fk_cascade_depth` although no cycle exists.

**Correct behavior:** cycle detection must be per-path (remove the table from
`visited` on branch return, or track the current recursion *stack*), with
row-level dedup of pending mutations to avoid double-deleting rows reachable
via both branches. The existing `CASCADE_DEPTH_LIMIT` already bounds true
cycles.

---

### 4. HIGH — built-in aggregates silently wrong over Dec/Big (and container) columns

**Where:** `crates/shamir-engine/src/query/read/aggregate.rs`:
- Sum arm `:387-401`, Avg arm `:403-417` — only `ScalarRef::Int` / `ScalarRef::F64` are accumulated; everything else silently skipped.
- Min `:418-468` / Max `:470-506` container fallback — comparison via `compare_values` at `:458` / `:498`.
- `compare_values` (`crates/shamir-engine/src/query/filter/resolve.rs:91-105`) has arms ONLY for Null/Bool/Int/F64/Str; Dec/Big/containers fall to `_ => None`.
- `ScalarRef` deliberately has no Dec/Big variant (`crates/shamir-types/src/record_view/scalar_ref.rs:26-27`), so `scalar_at` on a Dec field returns `None`.

**What I traced:** for a record field stored as `InnerValue::Dec` (which the
write path produces for every computed `$fn` math field — see
`write_helpers.rs:132-135` + `registry.rs:306-315`):

- **Sum** (`SelectItem::Aggregate`, `AggFunc::Sum`): `resolve_scalar` returns
  `None` for every row → nothing accumulated → `finish` returns **`Int(0)`**
  (`aggregate.rs:513-529`). No error, no Null — a confidently wrong zero.
- **Avg**: count stays 0 → returns `Null`.
- **Min/Max**: the container fallback materializes the Dec leaf into
  `OwnedExtreme::Tree(InnerValue::Dec)` for the FIRST row (`current: None →
  take = true`), but every subsequent comparison is
  `compare_values(Dec, Dec) == None` → `take = false` → the accumulator **keeps
  whatever row came first**. `min(price)`/`max(price)` over a Dec column both
  return the first-scanned row's value — order-dependent garbage, not an error.

Note the funclib `AggregateFn` path (`SelectItem::AggregateFn` → `agg.rs::to_dec`)
handles Dec correctly, so `sum` spelled as a funclib aggregate behaves differently
from the built-in `AggFunc::Sum` on the very same data.

**Correct behavior:** either accumulate Dec/Big via the materialize fallback
(as `fn_value_for_aggregator` at `aggregate.rs:804-825` already does for the
funclib path), or reject the aggregate with a type error. Silent 0/first-row is
the worst of the options.

---

### 5. HIGH — filters comparing against `$fn` math results (Dec) never match

**Where:**
- `crates/shamir-funclib/src/registry.rs:306-315` — `v_f64`/`v_dec`: every math scalar (`abs`, `round`, `pow`, `sqrt`, `mod`, …) returns `QueryValue::Dec`.
- `crates/shamir-engine/src/query/filter/resolve.rs:197-205` — `FnCall` results flow into the comparison layer unchanged.
- `crates/shamir-types/src/record_view/scalar_ref.rs:98-109` — `scalar_ref_cmp_qv` has no Dec arm (`_ => None`); same for `compare_values` (`resolve.rs:91-105`).
- `crates/shamir-engine/src/query/filter/filter_node.rs:305-323` — `Compare::matches`: a `(Some, Some)` pair whose comparison yields `None` makes `Eq/Gt/Gte/Lt/Lte` **false** and `Ne` **true**.

**Trigger:** filter
`{"op": "gt", "field": "price", "value": {"$fn": {"name": "abs", "args": [-100]}}}`.
`abs(-100)` resolves to `Dec(100)`; `scalar_ref_cmp_qv(Int(price), Dec(100))`
= `None` → the filter matches **no rows**, for every record, silently. With
`"op": "ne"` it matches **all** rows. The doc comment on `ScalarRef` frames
Dec as "not comparable in the current filter algebra", but combined with
funclib's decimal-first return convention, the emergent behavior is that the
entire `$fn` numeric-function surface is unusable on the RHS of a comparison —
and it fails silently rather than erroring.

**Same hole in `$expr`:** `eval_filter_expr`'s `as_f64`
(`resolve.rs:291-297`) accepts only Int/F64 — so
`{"$expr": {"op": "add", "args": [{"$fn": {...}}, 1]}}` (Dec operand) collapses
to `None` (absent) and the enclosing comparison is again silently false.

**Correct behavior:** add Dec (and Big) arms to `compare_values` /
`scalar_ref_cmp_qv` (Decimal↔Int is exact; Decimal↔F64 via the existing f64
fallback), and accept Dec in `$expr`'s numeric coercion — or normalize funclib
Dec results to Int/F64 at the filter boundary.

---

### 6. HIGH — ORDER BY over Dec/Big is lexicographic

**Where:** `crates/shamir-engine/src/query/read/order.rs:174-187`
(`QvSortKey::from_query_value`: `Dec(d) → Str(d.to_string())`,
`Big(b) → Str(b.to_string())`), compared as strings at `:277`.

**Trigger:** `ORDER BY price ASC` over a Dec column with values
`[9.5, 10.5, 2]` yields `[10.5, 2, 9.5]` (string order `"10.5" < "2" < "9.5"`).
Any Dec column whose values cross a digit-count boundary sorts wrongly.
Mixed Dec-vs-Int rows compare as `Str` vs `I64` → the `_ => Equal` arm →
arbitrary relative order. The comment says this "preserv[es] prior coercion
semantics" — i.e. the wrongness is inherited, not new, but it is still a wrong
numeric ordering presented as ORDER BY.

**Correct behavior:** map Dec to a numeric sort key (Decimal is `Ord`; add a
`Dec(Decimal)` variant to `QvSortKey`, with the existing Int/F64 cross-compare
extended to it).

---

### 7. MEDIUM — literal fast-path membership sets drop Int↔F64 coercion

**Where:** `crates/shamir-engine/src/query/filter/filter_node.rs`:
- `InSet::matches` `:362-385` — exact `TSet::contains` on the converted field value.
- `ContainsAnySet` `:555-571`, `ContainsAllSet` `:601-624` — `values.contains(item)` exact.
- Slow paths use `scalar_ref_cmp_qv` / `compare_values`, which treat `Int(1)` and `F64(1.0)` as Equal (`scalar_ref.rs:103-104`, `resolve.rs:99-100`).
- The `In` node even ships `set_contains_coercing` (`:47-76`) specifically to preserve coercion for column-ref sets — and its own comment (`:448-450`) acknowledges `InSet`'s divergence as a "known pre-existing difference".

**Trigger:** field `n = 1` (Int). Filter `{"n": {"$in": [1.0]}}` → all-literal →
`InSet` → `contains(F64(1.0))` fails on `Int(1)` → **no match**. Add any dynamic
element — `{"n": {"$in": [1.0, {"$param": "x"}]}}` — and the slow path's
`scalar_ref_cmp_qv` coercion makes the same value **match**. The same
literal-vs-dynamic sensitivity applies to `$contains_any`/`$contains_all`
(where it is not even acknowledged in a comment). A filter's answer should not
depend on whether its value list happens to be fully literal.

**Correct behavior:** use the same double-probe trick `set_contains_coercing`
already implements (probe `Int(n)` and `F64(n as f64)`) in `InSet`,
`ContainsAnySet`, and `ContainsAllSet`.

---

### 8. MEDIUM — user-registered scalar functions silently unavailable outside WHERE

**Where:** `FilterContext::new` defaults to a builtins-only resolver
(`crates/shamir-engine/src/query/filter/eval_context.rs:59-68`); the per-DB
resolver (with the `UserScalarLayer`) is threaded ONLY on the WHERE path
(`query_runner.rs:802-805`). Contexts built without `.with_scalars(...)`:
- SELECT function projections — `select_projection.rs:121-127` (`project_value`)
- group-SELECT scalar functions — `aggregate.rs:747-749`
- `when` guards — `query_runner.rs:156-161` (`resolve_skip`)
- sub-batch `bind` resolution — `query_runner.rs:347-350`
- ForEach `over` resolution — `query_runner.rs:499-502`

**Trigger:** register a user scalar `my_score` via `UserScalarLayer`
(`shamir-funclib/src/scalar_resolver.rs`). `WHERE { "$fn": {"name": "my_score", ...} }`
works; `SELECT { "$fn": {"name": "my_score", ...} }` on the same query returns
`Null` for every row (unknown_function → `resolve_filter_query` → `None` →
`unwrap_or(QueryValue::Null)` at `select_projection.rs:125`); a `when` guard
calling it silently evaluates the op as skipped. No error surfaces anywhere.

**Correct behavior:** thread `resolver.scalar_resolver()` into every context
that can evaluate `$fn` (the runner has it in scope at all five sites), or at
minimum surface an error rather than a silent Null/skip.

---

### 9. MEDIUM — `count_distinct` / `mode` wrong over Set/Map values

**Where:** `crates/shamir-funclib/src/agg.rs:197-215` (`CountDistinctAgg` uses
`compare::compare(...) == Equal` as equality), `:733-767` (`ModeAgg` sorts and
run-length counts with the same comparator);
`crates/shamir-funclib/src/compare.rs:64-65` — `Set`/`Map` compare **by `.len()`
only** (documented as "intentionally loose").

**Trigger:** `count_distinct` over the values `{"a": 1}` and `{"b": 2}` (two
different single-entry maps) returns **1**. `mode` over
`[{"a":1}, {"b":2}, {"b":2}]` can report a map that appears once as the mode
(all three compare Equal, so the run-length count sees one run of 3 and returns
the first). The looseness is documented on `compare` itself, but
`count_distinct`'s contract ("number of distinct values") is silently violated
for container values — no error, a plausible-looking wrong number.

**Correct behavior:** either implement structural equality for Set/Map in
`compare` (element-wise, as List already does) or make container inputs to
`count_distinct`/`mode` a `type_mismatch` error.

---

### 10. MEDIUM — self-referential FKs silently unenforced

**Where:** `crates/shamir-engine/src/query/batch/fk_actions.rs:765-769` and
`fk_on_update.rs:638-641` — discovery loops explicitly `continue` when
`name == parent_table_ref.table`, with a comment about avoiding infinite
recursion.

**Trigger:** classic `employees.manager_id → employees.id ON DELETE CASCADE`
(or RESTRICT). Deleting a manager performs no cascade, no restrict check, and
raises no error — subordinate rows keep a dangling `manager_id`. The declared
referential action is simply not applied. Skipping to avoid recursion is a
defensible MVP cut, but doing it *silently* (rather than rejecting the DDL or
the delete) converts a declared integrity constraint into a no-op.

---

### 11. MEDIUM — FK child matching is strict-typed (no Int↔F64 coercion)

**Where:** `fk_actions.rs:888-898` and `fk_on_update.rs:809-819`
(`scalar_ref_matches_qv`) — exact same-variant equality only.

**Trigger:** parent key stored as `Int(5)`, child FK field stored as `F64(5.0)`
(e.g. a client that sends all numbers as floats). Cascade/setnull/restrict scans
never match the child row → child survives the parent delete with a dangling
reference; RESTRICT fails to block. Filters elsewhere in the engine treat
`Int(5)` and `F64(5.0)` as equal (`scalar_ref_cmp_qv`), so FK enforcement is
stricter than query semantics — an inconsistency that manifests as silent
integrity loss rather than an error.

---

### 12. LOW — `$expr` `mod` unchecked remainder can panic

**Where:** `crates/shamir-engine/src/query/filter/resolve.rs:364-380` —
`FilterExprOp::Mod` guards `y == 0` but computes `x % y` directly, while the
sibling ops use `checked_add`/`checked_sub`/`checked_mul`/`checked_neg`.

**Trigger:** `{"$expr": {"op": "mod", "args": [-9223372036854775808, -1]}}`
(or field values reaching those operands) — `i64::MIN % -1` overflows: panic in
debug/overflow-checked builds. Should be `x.checked_rem(y)` mapped to `None`,
matching the other arms.

---

### 13. LOW — unchecked i64 arithmetic in accumulators / datetime

**Where:**
- `crates/shamir-engine/src/query/read/aggregate.rs:394` — `AggState::Sum`: `*sum_i += i` unchecked; a sum crossing ±2^63 panics (debug) or wraps (release), producing a wrong total with no error. The float path exists (`has_float` lift) but Int stays unchecked.
- `crates/shamir-funclib/src/datetime.rs:209-211` — `diff_secs`: `x - y` unchecked before `div_floor`.

**Correct behavior:** `checked_add`/`checked_sub` with lift-to-f64 (Sum) or
`out_of_range` (diff_secs).

---

### 14. LOW — funclib `compare` Int↔Big via f64 loses precision

**Where:** `crates/shamir-funclib/src/compare.rs:108-119` — any pair involving
`Big` (other than Big↔Big) converts both sides to `f64`.

**Trigger:** `compare(Int(i64::MAX), Big(i64::MAX - 1))` → both round to the
same f64 → `Equal`, although the values differ. Affects `min`/`max`/`between`/
`clamp` in funclib and `count_distinct` (two such values counted as one). The
f64 fallback is documented in the module header, but an exact BigInt↔i64 path
is cheap (`b.to_i64()` then integer compare, falling back to sign/magnitude).

---

### 15. LOW — cast functions reject `Big` inputs

**Where:** `crates/shamir-funclib/src/cast.rs:107-145` — `cast_to_int` /
`cast_to_dec` match Int/Bool/Dec/F64/Str; `Big` falls to `_ => cast_failed`,
even for a BigInt that fits i64/Decimal (contrast `agg.rs::to_dec:137-147`,
which does handle Big). `to_int({"$fn": ...})` chains that produce Big fail
where the sibling aggregate path would succeed.

---

## Non-findings (checked and found correct)

- `MedianAgg` even-N lower-median and `PercentileAgg` nearest-rank index math
  (including p=0 / p=1 edges) are correct and documented (`agg.rs:385-395,
  504-518`).
- `ModeAgg`'s run-length loop handles single-element, final-run, and
  first-run-wins ties correctly (`agg.rs:741-766`).
- The bytes-path pre-filter (`eval_bytes.rs`) is safe: its only trusted output
  is `Some(false)`, and every semantic gap (absent field, type mismatch, Dec,
  containers, Ext) conservatively returns `None`/full-decode. `IsNull`/`IsNotNull`
  absent-field semantics match the tree path's `is_null_at` (absent == null,
  `record_ref.rs:67-71`).
- `ValueCompare`'s 3-way null semantics are deliberate, documented, and
  test-pinned (`filter_node.rs:135-173`).
- Batch skip-cascade (`resolve_skip`), `return_only`/`return_all` filtering,
  ForEach pre-iteration gating (`TooManyIterations` before iteration 0), and the
  #666 deadline checkpoint placement (incl. the pre-commit check routing through
  the lock-releasing `Err` arm) all check out (`batch_execute.rs`,
  `query_runner.rs`).
- SQ8 quantizer algebra (dot expansion, L2 cancellation, NaN/clamp in
  `quantize`) is mathematically correct as documented (`sq8.rs`); the co-filter
  transient-None race has an explicit re-check fallback (`hnsw_adapter.rs:2107-2119`).
- `apply_defaults` / `apply_transforms` absence-vs-explicit-Null semantics and
  the `is_insert` gate are correct per their documented contract
  (`write_helpers.rs:161-266`).

## Suggested fix order

1. Finding 2 (FK on-update dedup) — one-line scoping fix, silent integrity loss.
2. Finding 1 (`$contains_all` duplicates) — small, wrong-`true` on the common path.
3. Finding 3 (diamond cascade false cycle) — per-path visited tracking + mutation dedup.
4. Findings 4/5/6 together — one coherent "Dec-aware comparison layer" task
   (add Dec/Big arms to `compare_values`/`scalar_ref_cmp_qv`, Dec handling in
   aggregate accumulators, numeric `QvSortKey::Dec`).
5. Findings 7/8 — coercing set probes; thread the real ScalarResolver.
6. Remaining MED/LOW as cleanup tasks.
