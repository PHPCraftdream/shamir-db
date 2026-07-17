# CI regression — `$fn`-with-`$ref` computed values on write broke after #641

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The regression (confirmed via GitHub Actions, all 3 OSes, deterministic — not flaky)

`crates/shamir-db/tests/functions_e2e.rs`'s `seed_users()` helper (used by
4 tests: `e2e_computed_value_persisted_on_insert`,
`e2e_filter_with_fn_call`, `e2e_group_by_library_aggregate`,
`e2e_select_scalar_function`) inserts rows like:

```
doc! {
    "name" => "alice",
    "city" => "NYC",
    "age" => 30,
    "email" => "A@X.COM",
    "email_norm" => func("strings/lower", [col("email")]),
}
```

`col("email")` builds a `FilterValue::FieldRef` (`$ref`) pointing at the
SIBLING field `"email"` in the SAME row being inserted — a same-document
computed value (`email_norm` = lowercase of `email`), one of the engine's
documented "computed value on write" features
(`crates/shamir-engine/src/table/write_helpers.rs`'s
`resolve_computed_record`/`eval_write_value`, doc comment: *"A field whose
value is `{ "$fn": { "name": "strings/lower", "args": [{ "$ref": ["email"]
}] } }` is evaluated through the scalar registry; `$ref` arguments resolve
against the record's literal (non-computed) fields."*).

This now fails on every OS with:

```
QueryError { alias: "ins", message: "marker Map({\"$fn\": Map({\"name\":
Str(\"strings/lower\"), \"args\": List([Map({\"$ref\":
List([Str(\"email\")])})])})}) failed to resolve (unknown alias/function,
or a nested reference that itself did not resolve)", code:
Some("malformed_marker") }
```

### Root cause (confirmed by tracing the exact code path)

Commit `c286b199` ("feat(engine): #641 — resolve `$query`/`$fn`/`$cond`/
`$expr` inside write values") added a NEW write-value marker resolver at
the **batch/query-dispatch level**
(`crates/shamir-engine/src/query/batch/param_subst.rs`'s
`resolve_write_value`, invoked from `query_runner.rs`'s `BatchOp::Insert`
dispatch, ~line 820) that now runs UNCONDITIONALLY for any Insert value
containing ANY of the 5 reserved markers, INCLUDING `$fn`. This resolver's
own doc comment already documents (lines 24-38) that `$ref` is
"explicitly OUT OF SCOPE" for this resolver — it evaluates every marker
against a `DUMMY_RECORD: InnerValue = InnerValue::Null` (no real row
data), so a `$ref` inside a `$fn`'s args always misses and the WHOLE
`$fn` call is treated as unresolvable, producing the hard
`WriteValueError::MalformedMarker` above.

**Before #641**, this batch-level resolver only handled bare `$param`
markers (nothing else) — so a top-level `{"$fn": ...}` field value passed
through UNTOUCHED to the table layer, where `TableManager::execute_insert_tx`
(`crates/shamir-engine/src/table/write_exec.rs`) calls
`resolve_computed_record` (`write_helpers.rs:93`), which builds a `literal`
map of the row's OWN non-computed sibling fields (line 108) and evaluates
the `$fn` call's `$ref` args against THAT real per-row context via
`eval_write_value` (line 132) — correctly producing the lowercased email.
**This older mechanism is still fully present and correct in the
codebase** — it was never touched by #641 — but #641's new batch-level
resolver now runs FIRST and hard-errors before the table layer ever gets
the value.

Confirmed this is a real, deterministic regression (not environment
flakiness): identical failure on ubuntu-latest, macos-latest, and
windows-latest, at the exact same 4 tests, same error message.

## The fix

**Scope precisely to the ACTUAL table-layer feature, don't overreach.**
`is_computed_field` (`write_helpers.rs:71`) only recognizes a TOP-LEVEL
`{"$fn": ...}` field value (checks `m.contains_key("$fn")`) — `$cond`/
`$expr` wrapping a `$fn`+`$ref` was NEVER supported by the table layer
either before or after #641 (out of scope for this fix; don't try to
extend that). The fix is: when `resolve_write_value`
(`param_subst.rs`) is about to resolve a `$fn` marker, check whether that
`$fn` call's `args` contain a `FilterValue::FieldRef` (`$ref`) ANYWHERE,
recursively (an arg could itself be a nested `$fn`/`$expr`/`$cond`
containing a `$ref` deeper down). If so, **do NOT attempt resolution at
all** — leave that field's value COMPLETELY UNCHANGED (the original
`QueryValue`, marker map intact), so the value continues on to the table
layer's `resolve_computed_record`, which already knows how to resolve it
correctly against the real row.

If NO `$ref` is found anywhere inside the `$fn` (the common #641 case —
e.g. `{"$fn": {"name": "math/abs", "args": [{"$query": "@other[0].x"}]}}`),
proceed with resolution EXACTLY as #641 already does — this fix must NOT
regress #641's own new capability.

### Implementation sketch (adapt as needed; use your own judgement on the
### cleanest way to wire this into the existing code)

1. Add a small recursive helper in `param_subst.rs` (or reuse/adapt
   `filter_value_contains_field_based_comparison`-style walking from
   `shamir-query-types`'s `planner.rs` if there's a clean way to share
   it — but a small local helper in `param_subst.rs` is also fine, don't
   force a cross-crate reuse if it's awkward), e.g.
   `fn filter_value_contains_field_ref(fv: &FilterValue) -> bool`,
   returning `true` iff `fv` is `FilterValue::FieldRef` itself, OR
   recursively any of: `FnCall.args()`, `Expr.args`, `Cond`'s
   `condition`/`then`/`or_else` (condition is a `Filter` — you'll need a
   parallel `Filter`-level walker that inspects `Filter`'s own
   `FilterValue` operands, OR simplify by only recursing into
   `then`/`or_else` and treating any `Cond` as "assume it might contain a
   ref" if walking `Filter`'s internals is disproportionate — use your
   judgement, but justify the choice in your summary), `Array` elements.
2. In `resolve_write_value_inner` (`param_subst.rs`), in the
   `is_marker_map(map)` branch, AFTER decoding the marker into `fv:
   FilterValue` but BEFORE calling `resolve_filter_query`: if `fv` is
   `FilterValue::FnCall { call }` and
   `call.args().iter().any(filter_value_contains_field_ref)`, return
   `Ok(value.clone())` (pass the ORIGINAL marker map through unresolved)
   instead of attempting resolution. Every other marker type (`$query`,
   `$param`, `$cond`, `$expr`) keeps its EXISTING behavior unchanged (do
   not extend the pass-through to `$cond`/`$expr` — the table layer
   doesn't support those anyway, so leaving them to error as today is
   correct and matches the documented pre-#641 scope).
3. Double check `contains_param_ref`'s (`param_subst.rs:90`) fast pre-scan
   still correctly triggers the SLOW path (`resolve_write_value_inner`)
   for a value containing a `$fn`+`$ref` marker even though that marker
   ends up being passed through unresolved — i.e. don't try to
   "optimize" the fast-path pre-scan to skip `$fn`+`$ref` values
   entirely; it must still route through the resolver (which recurses
   into sibling fields of the SAME row that might have real `$query`/
   `$param` markers needing resolution) — the pass-through only applies
   to the SPECIFIC `$fn`+`$ref` marker itself, not the whole row.

## Tests

1. **Confirm the exact regression is fixed**: `./scripts/test.sh -p
   shamir-db --full -- functions_e2e` must show all 4 previously-failing
   tests passing (`e2e_computed_value_persisted_on_insert`,
   `e2e_filter_with_fn_call`, `e2e_group_by_library_aggregate`,
   `e2e_select_scalar_function`) — these are pre-existing tests, not new
   ones; you are fixing production code to make them pass again, not
   editing the tests (unless you find the tests themselves need a small,
   clearly-justified adjustment — if so, explain exactly why in your
   summary).
2. **New regression test for the specific interaction**: add a test
   (in `crates/shamir-engine/src/query/batch/tests/executor_tests/
   write_value_resolution_tests.rs` — the existing #641 test file,
   check its structure first) proving a `$fn`+`$ref` write-value marker
   passes through `resolve_write_value` UNCHANGED (e.g. assert the
   resolved `QueryValue` still has the original `{"$fn": ...}` shape, or
   test at the `resolve_write_value` function level directly), AND a
   companion test proving a MIXED case still works: a row with BOTH a
   `$fn`+`$ref` field (left for the table layer) AND a genuine `$param`
   or `$query` field (resolved by `resolve_write_value` as #641 already
   does) in the SAME insert — both must end up correct after the full
   insert pipeline (this may be best as an engine-level integration test
   using the existing `execute_batch` test harness rather than a unit
   test on `resolve_write_value` alone, if that's a cleaner way to prove
   the end-to-end interaction — use your judgement).
3. **Non-regression**: re-run the EXISTING #641 tests in
   `write_value_resolution_tests.rs` (the ones proving `$query`/`$param`/
   `$cond`/`$expr` resolution in write values) — they must all still pass
   unchanged; in particular the existing test noted in your own
   exploration as "`$fn` with a literal argument (no `$ref`...)" must
   still pass exactly as before (no `$ref` present → full resolution as
   before, unaffected by this fix).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-db --full` green (the full functions_e2e
  suite + everything else in that crate).
- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full`
  green (no regression in the engine's own write-value tests).
- `cargo fmt --all -- --check` clean (or scoped to touched crates if a
  full-workspace check is slow — but report which you ran).
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Explain, in your own words, why this fix does NOT reopen any part of
  the silent-fold class of bug #651/#663 closed earlier — i.e. this is
  a "defer to the correct downstream resolver" pass-through, not a
  "silently treat as always-true/always-false" fold; walk through why
  the two are different.

## Out of scope

- Do NOT attempt to make `$ref` resolvable at the batch/query-dispatch
  level (e.g. by threading real per-row context through
  `resolve_write_value`) — that is explicitly out of scope per
  `param_subst.rs`'s own existing doc comment and would be a much larger
  feature change. The correct fix is deferring to the EXISTING,
  already-correct table-layer mechanism, not reinventing it.
- Do NOT extend `is_computed_field`/`resolve_computed_record` to support
  `$cond`/`$expr`-wrapped `$fn`+`$ref` calls — out of scope, not part of
  the regression (it never worked before #641 either).
- Do NOT touch anything from this session's earlier #661-670 wave.
