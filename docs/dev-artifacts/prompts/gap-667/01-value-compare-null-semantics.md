# #667 — document and test `Filter::ValueCompare`'s null-comparison semantics

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`FilterNode::ValueCompare`'s `matches()` implementation
(`crates/shamir-engine/src/query/filter/filter_node.rs:283-302`) resolves
both operands independently via `resolve_filter_query` and then branches:

```rust
FilterNode::ValueCompare { left, op, right } => {
    let lhs = resolve_filter_query(left, record, ctx);
    let rhs = resolve_filter_query(right, record, ctx);
    match (&lhs, &rhs) {
        (Some(a), Some(b)) => match op {
            CompareOp::Eq => compare_values(a, b) == Some(Ordering::Equal),
            CompareOp::Ne => compare_values(a, b) != Some(Ordering::Equal),
            CompareOp::Gt => compare_values(a, b) == Some(Ordering::Greater),
            CompareOp::Gte => matches!(compare_values(a, b), Some(Ordering::Greater | Ordering::Equal)),
            CompareOp::Lt => compare_values(a, b) == Some(Ordering::Less),
            CompareOp::Lte => matches!(compare_values(a, b), Some(Ordering::Less | Ordering::Equal)),
        },
        (None, _) | (_, None) => matches!(op, CompareOp::Ne),
    }
}
```

`compare_values` (`crates/shamir-engine/src/query/filter/resolve.rs:81-95`):

```rust
match (a, b) {
    (Value::Null, Value::Null) => Some(Ordering::Equal),
    (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
    ... (other same-type pairs) ...
    _ => None,
}
```

Reading these two together, `ValueCompare` actually has **THREE distinct
"nothing to compare" shapes**, only one of which behaves the way a reader
might naively guess, and NONE of which is documented or tested anywhere:

1. **Genuinely unresolvable operand** — `resolve_filter_query` itself
   returns `None` (an unbound `$query` alias, a `$fn` call that errored, an
   unbound `$param`, etc. — an ABSENCE, not a value). Hits the outer
   `(None, _) | (_, None)` arm: **only `Ne` matches (`true`); every other
   op (`Eq`/`Gt`/`Gte`/`Lt`/`Lte`) is `false`.**
2. **Both operands resolve to the LITERAL value `null`** (e.g. both sides
   are `FilterValue::Null`, or a `$query` ref whose target field is
   genuinely `null`) — this is `Some(QueryValue::Null)` on BOTH sides, so
   it reaches the INNER `(Some(a), Some(b))` arm and calls
   `compare_values(&Null, &Null)`, which explicitly returns
   `Some(Ordering::Equal)`. **`Eq`/`Gte`/`Lte` are `true`; `Ne`/`Gt`/`Lt`
   are `false`.** This is the OPPOSITE of case 1's `Eq=false, Ne=true` —
   an explicit, resolved `null` on both sides is treated as a genuinely
   COMPARABLE, EQUAL value — closer to JS's `null === null` than to SQL's
   three-valued `NULL = NULL` (which is `UNKNOWN`, not `TRUE`).
3. **One operand resolves to the literal value `null`, the other to a
   non-null value of a different type** (e.g. `left` resolves to
   `Some(QueryValue::Null)`, `right` to `Some(QueryValue::Int(5))`) — both
   sides ARE `Some(..)`, so this ALSO reaches the inner arm, but
   `compare_values(&Null, &Int(5))` falls through to the `_ => None` catch-
   all (no same-type arm matches). **`Ne` is `true` (`None != Some(Equal)`);
   every other op is `false`.** Outwardly IDENTICAL to case 1's result
   shape, but reached via a completely different code path (a resolved
   type MISMATCH, not an absent operand) — worth documenting as a
   distinct case even though the immediate boolean results happen to
   coincide, because a reader auditing `compare_values` alone (without the
   `ValueCompare` wrapper) needs to know this is intentional, not an
   oversight.

Nothing in the codebase currently explains this 3-way distinction (case 2
in particular is easy to miss — it's the ONE case where an operand that
LOOKS like "nothing" is actually treated as a genuine, self-equal value),
and zero tests exercise ANY of it — `grep -rln "ValueCompare"
crates/shamir-engine/src` finds test files exercising `ValueCompare` for
ordinary numeric comparisons (`when_skip_tests.rs`'s
`value_compare_makes_balance_gte_amount_scenario_work_*` tests) but NONE
touching a `Null`/unresolvable operand on either side.

## The fix — docs + tests only, NO behavior change

This task does NOT change `matches()`'s or `compare_values`'s logic — the
semantics described above are being ADOPTED as the documented, tested
contract, not altered. If you find yourself wanting to change behavior,
STOP — that would be a new, separate task (changing the return type of an
established comparison operator is a behavior change with real
compatibility implications, not something to fold into a docs+tests-only
brief).

### Part A — doc comment

Extend the doc comment on `FilterNode::ValueCompare`
(`crates/shamir-engine/src/query/filter/filter_node.rs:124-135`, right
above the variant definition) to explicitly describe the 3-way null
semantics from the Background section above (genuinely-unresolvable vs.
both-literal-null vs. null-vs-non-null-type-mismatch), in your own words,
concise but precise — a future reader should be able to predict the
boolean result for each of the 3 cases × 6 operators without reading
`compare_values`'s source. Cross-reference `compare_values`
(`crates/shamir-engine/src/query/filter/resolve.rs:81`) so the two pieces
of documentation stay linked. Also add a short note to `compare_values`
itself (`resolve.rs:81`) pointing out that `(Null, Null) => Some(Equal)`
is a DELIBERATE choice callers rely on (specifically `ValueCompare`) —
not an oversight that happened to fall out of the match arms' ordering.

### Part B — tests

Add to `crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs`
(the existing home for `ValueCompare`-via-`when` integration tests — follow
its established pattern: build a `BatchRequest` with a `when: Some(Filter::ValueCompare{..})`
on a probe op, execute it, and assert `skipped`/not-`skipped` on the
result) OR, if a more unit-level test is a better fit, add a new file
`crates/shamir-engine/src/query/filter/tests/value_compare_null_tests.rs`
(wire it into that directory's `tests/mod.rs`) calling
`FilterNode::matches`/`resolve_filter_query` directly — use your judgment
for whichever is more natural given the existing test infrastructure in
each location; a mix (a couple of direct unit tests for precision, plus
one `when`-level integration test for the end-to-end shape) is also fine.

Cover, for AT LEAST `Eq` and `Ne` (the two ops where the 3-way distinction
actually produces different-looking outcomes) and ideally also `Gte`/`Lte`
(to show the "both null → Equal counts" effect on the inclusive ordering
ops too):

1. **Genuinely unresolvable, both sides** (e.g. both operands are
   `FilterValue::QueryRef` to aliases that don't exist / weren't provided
   in `resolved_refs` — or simpler, both `FilterValue::Param` referencing
   unbound param names): assert `Eq` is `false`, `Ne` is `true`, `Gte`/`Lte`
   are `false`.
2. **Genuinely unresolvable, ONE side only** (e.g. `left` is a literal
   `FilterValue::Int(5)`, `right` is an unbound `$param`): assert the SAME
   shape as case 1 (`Eq=false, Ne=true, Gte/Lte=false`) — proving the
   "either side absent" branch doesn't distinguish left-absent from
   right-absent from both-absent.
3. **Both sides literal `null`** (`left: FilterValue::Null, right:
   FilterValue::Null`): assert `Eq=true`, `Ne=false`, `Gte=true`,
   `Lte=true` — THE decisive case proving `null` is treated as a
   genuinely self-equal value, not as "absent".
4. **One side literal `null`, other side a real non-null value** (`left:
   FilterValue::Null, right: FilterValue::Int(5)`): assert `Eq=false`,
   `Ne=true`, `Gte=false`, `Lte=false` — same outward shape as case 1/2 but
   via the type-mismatch path through `compare_values`, not the
   None-operand path; a comment on this test should note WHY it's still
   worth a separate test from case 1 (different code path, same result —
   the distinction matters for anyone auditing `compare_values` in
   isolation).
5. **Regression/sanity**: re-confirm an ordinary same-type comparison
   (e.g. `Int(100) Gte Int(40)`) still behaves as before — this should
   already be covered by the existing `value_compare_makes_balance_gte_amount_scenario_work_*`
   tests; only add this if you determine those don't already give
   adequate coverage as a baseline next to the new null-focused tests.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all new
  tests.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Confirm in your own words that test 3 (both-null) and test 1/2/4
  (unresolvable / type-mismatch) produce OPPOSITE `Eq`/`Ne` outcomes,
  and that this is the intended, now-documented behavior — not a bug you
  found and should fix.

## Out of scope

- Do NOT change `matches()`'s or `compare_values`'s actual logic/return
  values — this is a documentation + test-coverage task only.
- Do NOT touch #661/#662/#663/#665/#666 — separate, already-completed
  tasks.
