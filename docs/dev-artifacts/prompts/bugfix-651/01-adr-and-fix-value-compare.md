# #651 — CRITICAL: `when` field-based comparisons always fold to a fixed result

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (already root-caused, do not re-investigate from scratch)

`QueryRunner::resolve_skip` (`crates/shamir-engine/src/query/batch/query_runner.rs:102-140`)
evaluates a `QueryEntry.when: Option<Filter>` against an EMPTY SYNTHETIC
RECORD (`InnerValue::Null`) through a freshly-created, empty scratch
`Interner::new()`:

```rust
let scratch = shamir_types::core::interner::Interner::new();
let ctx = FilterContext::new(&scratch, resolved_refs)...;
let node = crate::query::filter::compile_filter(filter, &scratch);
!node.matches(&InnerValue::Null, &ctx)
```

`compile_filter` (`crates/shamir-engine/src/query/filter/compile.rs`)
resolves EVERY comparison operator's `field: FieldPath` via
`intern_field_path_compact(field, interner)` — a lookup-only call
(`interner.get_ind(part)`, never an insert). Against a fresh, empty scratch
interner this ALWAYS returns `None` for any field name, so
`compile_compare` (line 282-297, used by `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/
`FieldEq`) unconditionally folds to `FilterNode::False` — regardless of
the RHS `value: FilterValue`, even when that RHS carries real cross-query
data via a `$query` ref. `IsNull` always folds to `True`, `IsNotNull`
always folds to `False`, for the identical reason.

Net effect: the ADR's own canonical motivating scenario — "run this op iff
`$query_ref_A >= $query_ref_B`" (e.g. "debit iff balance >= amount") — is
NOT expressible through any field-based `Filter` variant today.
`docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`'s own
design note says `when` should support "$query/$fn/$param/literals" value
comparisons with "no `FieldRef` support needed/meaningful" — but no `Filter`
variant today implements a field-FREE value-vs-value comparison; every
comparison variant is hard-wired to a `field: FieldPath` (`Vec<String>`,
`crates/shamir-query-types/src/filter/filter_enum.rs:14-45`) meant to
address a REAL record's field, which structurally cannot exist in a
record-free `when` evaluation context.

This was independently re-confirmed twice this session (the Epic03/E e2e
test's header, `crates/shamir-client/tests/batch_when_e2e.rs`, documents
a reverted probe that reproduced it directly).

## Root architectural cause

`Filter`'s comparison variants (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq`)
are, by design, a RECORD-field-vs-value shape (`field: FieldPath, value:
FilterValue`) — correct and necessary for WHERE clauses (real per-row
filtering) and MUST NOT change for that use case (zero regression risk
there). `when` was fit into this same `Filter` type as a reuse of
convenience, but `when` has no record to compare a field against — it
needs a genuinely different shape: a VALUE-vs-VALUE comparison, where BOTH
sides are `FilterValue` (each independently resolvable via `$query`/`$fn`/
`$param`/literal through `resolve_filter_query`, exactly like Epic02's
`$cond`/`$expr` machinery already does for `FilterValue::Expr`).

## Chosen fix direction (decided by the orchestrator — implement exactly this, do not re-litigate)

1. **Add a new `Filter` variant**, `Filter::ValueCompare { left: FilterValue,
   op: ValueCompareOp, right: FilterValue }` (name the op enum
   `ValueCompareOp` with variants `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte` — reuse
   `crate::query::filter::filter_node::CompareOp`'s existing 6 variants if
   it's visible/reusable from `shamir-query-types`, otherwise define an
   equivalent in `shamir-query-types::filter`). This variant compares TWO
   independently-resolved `FilterValue`s — no field path, no record
   dependency. It is meaningful in ANY `Filter`-evaluation context
   (`when`, and in principle a real WHERE clause too, though its primary
   motivating use is `when`), unlike the existing field-based variants
   which remain UNTOUCHED, still record-field-based, still used for real
   per-row WHERE-clause filtering with zero behavior change.
2. **`compile_filter`/`FilterNode` gains a matching `Compare` variant that
   evaluates by calling `resolve_filter_query` (or the engine's
   equivalent) on BOTH `left` and `right` against the CURRENT `record`/`ctx`
   at match time (not compile time — unlike field-based comparisons, this
   one can't be constant-folded at compile time since $query refs resolve
   from `ctx.resolved_refs`, which varies per call), then compares the two
   resulting `QueryValue`s using the same numeric/string comparison
   semantics `compile_compare`'s existing `FilterNode::Compare` uses.
   Read `crates/shamir-engine/src/query/filter/filter_node.rs`'s existing
   `FilterNode::Compare`/`matches` implementation and mirror its comparison
   logic (don't reinvent number/string ordering rules).
3. **`resolve_skip` gains a defensive check**: if `entry.when` contains ANY
   of the OLD field-based comparison variants (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/
   `Lte`/`FieldEq` — NOT `IsNull`/`IsNotNull`, which remain a legitimate
   presence-guard pattern against the synthetic record, and NOT `And`/`Or`/
   `Not`/`ValueCompare`), return an EXPLICIT ERROR (`BatchError`, add a new
   variant — check `batch_error.rs` for the existing naming convention,
   e.g. `InvalidWhenFilter { alias: String, message: String }`) at
   plan-time or execution-time BEFORE silently evaluating to a fixed
   result. This is the single most important safety fix here: today a
   caller who (reasonably, given the ADR's own docs) writes `when:
   Filter::Gte { field: vec!["balance"], value: ... }` gets a SILENT WRONG
   ANSWER; after this fix they get a clear error telling them to use
   `Filter::ValueCompare` instead. Write the error message to explicitly
   name the fix: "field-based comparisons are not meaningful inside `when`
   (no record exists) — use Filter::ValueCompare for value-vs-value
   comparisons instead".
   - Decide where this check belongs: either at `BatchPlanner::plan` time
     (preferred — fails fast before any execution, consistent with other
     `BatchError` plan-time validations) or at `resolve_skip` call time
     (acceptable fallback if plan-time is structurally awkward — e.g. if
     the planner doesn't currently walk into `Filter` trees deeply enough;
     check `planner.rs` for whether it already has a `Filter`-walking
     utility to extend, vs. adding a new one from scratch).
4. **Builders**: add a Rust query-builder helper (`crate::val` or wherever
   `Filter` construction helpers live —
   check `crates/shamir-query-builder/src/` for the existing `filter::` or
   `val::` module) for `Filter::ValueCompare`, e.g. `value_gte(left, right)`
   /`value_eq(left, right)` etc., and the TS equivalent
   (`crates/shamir-client-ts/src/core/builders/filter.ts` or wherever
   `filter.isNull`/`filter.gte` etc. live) — `filter.valueGte(left, right)`
   etc. Match the existing naming/signature conventions in those files.
5. **Tests**: add unit tests proving the NEW `ValueCompare` actually makes
   the ADR's canonical scenario work: a `when: Filter::ValueCompare { left:
   $query-ref-to-balance, op: Gte, right: $query-ref-to-amount }` correctly
   runs when balance >= amount and skips when it doesn't, over BOTH
   directions (like the existing `batch_when_e2e.rs`'s two-scenario
   pattern, but now with the REAL data-driven condition, not the
   `IsNull`/`IsNotNull` workaround). Also add a unit test confirming the
   OLD field-based variants now produce the new explicit error inside a
   `when` context instead of silently folding.
6. **E2E**: update or add to `crates/shamir-client/tests/batch_when_e2e.rs`
   and `crates/shamir-client-ts/src/__tests__/e2e-when.test.ts` — replace
   (or add alongside, your call, but the canonical "balance >= amount"
   scenario MUST be proven over a real wire round-trip using the NEW
   `ValueCompare`, not the `IsNull`/`IsNotNull` workaround) at least one
   real data-driven scenario. Update both files' header doc-comments to
   reflect that #651 is FIXED (remove or update the "KNOWN ENGINE BUG"
   sections that currently describe the workaround).
7. **Docs**: update `docs/guide-docs/guide/01-queries.md`'s `when`/`switch`
   section (currently has a prominent ⚠️ warning block about #651, added in
   commit `6ab3a573`) to reflect the fix — replace the warning with
   guidance on `Filter::ValueCompare`/`valueGte` etc. for data-driven
   conditions, and note that the old field-based variants now error
   clearly inside `when` instead of silently misbehaving.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p
  shamir-query-builder --full` green.
- `./scripts/test.sh -p shamir-client --full -- when` green (e2e).
- TS: relevant vitest suites green (builder tests + the e2e-when test file).
- `cargo fmt --check` clean for every touched crate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace — this session repeatedly found that growing an enum/struct
  breaks some OTHER crate's construction/match site).
- Report literal command output for all of the above.

## Out of scope

- Do NOT touch the existing field-based comparison variants' behavior for
  real WHERE-clause (per-row) filtering — zero behavior change there.
- Do NOT attempt #660, #641, #643, #634, #659 here — separate tasks.
- If you discover the plan-time validation location is structurally hard
  (e.g. the planner doesn't walk `Filter` trees today), it's fine to
  implement the check at `resolve_skip` call time instead — note this
  explicitly in your summary as a documented trade-off, don't silently
  downgrade the safety guarantee without saying so.
