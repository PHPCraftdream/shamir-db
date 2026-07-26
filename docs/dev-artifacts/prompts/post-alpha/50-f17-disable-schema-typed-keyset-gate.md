# Brief for F-17 (#810, P0) — disable F-1's schema-typed keyset gate; fall back to Offset

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context: a real, three-times-independently-confirmed gap in F-1 (#792)

F-1 (#792, part of the earlier "Wave F" hardening campaign) added a gate,
`order_by_column_is_schema_typed_scalar`
(`crates/shamir-server/src/db_handler/cursor_handlers.rs:475-510`), that lets
a keyset cursor trust a column's type homogeneity when a schema rule with a
non-container scalar `TypeTag` (`Int`/`Bool`/`String`/`Bin`) is bound to that
field. The call site is `cursor_handlers.rs:1237-1251`: when the gate returns
`true`, `mode` stays `PaginationMode::Keyset`; when `false`, it's forced to
`PaginationMode::Offset`.

**The gap:** `set_table_schema`/`add_schema_rule`
(`crates/shamir-db/src/shamir_db/execute/admin_schema.rs`) validate the new
rule's DTO shape and its FK/unique index requirements, but they **never scan
or validate pre-existing rows** against the new rule. So the gate's implicit
claim — "every row satisfying this schema is provably homogeneous by
construction" — is only true for rows written AFTER the schema was bound. A
schemaless table that already has mixed-type data in a field, followed by an
operator declaring a schema rule for that field, ends up with the gate
returning `true` (Keyset enabled) even though older rows aren't provably
homogeneous. `compare_values` in `shamir-engine`'s `resolve.rs` has no
comparison arm for a value of the "wrong" type reaching the keyset boundary
filter, so those older rows can silently vanish from later pages — the EXACT
bug class F-1 was written to close, recurring through an unenforced
assumption.

This was found independently by **three separate reviews** of the same
codebase state (see `docs/dev-artifacts/research/2026-07-26-wave-f-consolidated-synthesis/SYNTHESIS.md`
for the full cross-reference): an `@oh` agent review, a `/crush` agent
review, and a deeper static audit
(`docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`, finding
R1). The orchestrator personally verified the claim by reading
`admin_schema.rs`'s rule-handling code directly — there is no backfill/
existing-data validation path anywhere in it.

Related: the third review also flagged that `TypeTag::Bin` is accepted by
this same gate even though `safe_seek_key` (`cursor_handlers.rs:675`) always
returns `None` for `QueryValue::Bin` (no `compare_values` arm exists for it)
— Bin never actually benefits from Keyset mode, it just pays for a wasted
null-probe before falling back to Offset anyway. **That narrower `Bin`
finding is tracked as a SEPARATE follow-up task (F-18, #811) — do NOT fix it
here.** This brief is scoped to the historical-row-homogeneity gap only.

## Decision (already made — implement, don't re-derive)

The proper fix (a durable "schema validated through version N" marker,
checked against the cursor's pinned snapshot version, possibly combined with
a one-time backfill-validation scan when a schema rule is added to a
non-empty table) is a substantially larger design — tracked as a **separate,
future, post-alpha task**, not this one.

**For this task (matching the third review's own "safest alpha fix"
recommendation): disable the schema-typed-scalar gate's POSITIVE branch
entirely.** `order_by_column_is_schema_typed_scalar` must always return
`false`, so every keyset-mode candidate — schema-typed or not — falls back
to `PaginationMode::Offset`, exactly like a schemaless column does today.
This fully closes the P0 (no more silent row loss risk from this mechanism)
by retracting the "CLOSED for schema-typed scalar columns" claim, at the
cost of losing the (real, measured) keyset-pagination performance win F-1
delivered for schema-typed columns — an acceptable, honest tradeoff for
alpha, to be revisited by the future validated-through-version design.

### Implementation

1. In `crates/shamir-server/src/db_handler/cursor_handlers.rs`, change
   `order_by_column_is_schema_typed_scalar` so it unconditionally returns
   `false` (simplest: replace the function body with `false`, OR keep the
   lookup logic but short-circuit before the final `matches!` — your call,
   but prefer the simplest correct change; do not delete the function if
   other code still calls it, just neuter its positive path). Update its doc
   comment to state plainly: this gate is temporarily disabled pending a
   durable validated-through-version design (cite F-17/#810 and the
   consolidated synthesis doc path above) — it no longer trusts a bound
   schema rule's type as proof of historical-row homogeneity, because schema
   rules are not retroactively enforced against pre-existing rows.
2. Update the call site's surrounding comment in `cursor_handlers.rs` (around
   line 1227-1236) if it makes claims that are no longer true now that the
   gate always returns `false`.
3. Update `docs/guide-docs/KNOWN_LIMITATIONS.md` §6 (search for "CLOSED for
   schema-typed scalar columns (F-1, #792)"): change that bullet to say the
   mechanism is now DISABLED (not closed), citing F-17/#810, and that mixed
   `QueryValue` type in one `ORDER BY` column is STILL OPEN for ALL columns
   (schema-typed or not) until the validated-through-version design lands.
   Keep the rest of that section's structure intact (the `Null`/`Dec`/`Big`/
   `NaN`/W-7 sub-bullets are unrelated and must not be touched).

### Tests — this is the bulk of the work

`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs` has
several existing tests that positively assert `PaginationMode::Keyset` for
schema-typed columns — these were written FOR F-1 and will now correctly
regress once the gate always returns `false`. Known sites (search for
`PaginationMode::Keyset,` to find the exact current set, there may be more
than listed here — re-verify against the file, don't trust this list
blindly):

- `null_probe_regression_non_null_column_stays_keyset` (~line 3164) — uses
  `build_handler_with_scores`, which binds a schema (`TypeTag::Int` on
  `score`) via the `bind_schema` test helper. This test's purpose is to
  guard the NULL-probe mechanism, not the schema-typed gate specifically —
  update its `PaginationMode::Keyset` assertion to `PaginationMode::Offset`
  and adjust its doc comment/assertion message to explain why (the gate is
  now disabled), while preserving the actual regression check (every row
  returned exactly once, no duplication) since that must still hold under
  Offset mode too.
- `bin_order_by_value_uses_offset_fallback_not_silent_drop` (~line 3628) —
  update the SAME way; note its message already says "offset fallback" in
  its own name/some assertions, so check whether it needs to change at all
  or just its `PaginationMode` expectation (it may already expect Offset for
  a *different* reason — read carefully before editing).
- A `Str` ORDER BY test (~line 3794, look for "a Str ORDER BY column was
  already safe pre-this-task") — same treatment.
- `int_order_by_regression_still_uses_real_keyset_seek` (~line 3841) — same
  treatment; note the function name itself claims "real keyset seek", which
  will need renaming since it's about to test the Offset path instead.
- `schema_typed_int_required_order_by_uses_keyset_mode` (~line 3953) — same
  treatment; this test's very NAME is now wrong (it will use Offset, not
  Keyset) — rename it to reflect the new behavior (e.g.
  `schema_typed_int_required_order_by_falls_back_to_offset`).
- Any OTHER test asserting `PaginationMode::Keyset` for a schema-typed setup
  discovered while doing this — grep thoroughly, don't stop at this list.

For each updated test: change the assertion to `PaginationMode::Offset`,
update its doc comment / assertion failure message to explain the new
"gate disabled" reasoning (cite F-17/#810), but PRESERVE the underlying
data-correctness assertions (every row returned exactly once, in order,
no duplication/loss) — Offset mode must still be provably correct for these
scenarios, this task doesn't touch Offset-mode correctness at all, only
which mode gets selected.

**New regression test required:** add ONE new test reproducing the exact
scenario from the finding: (1) create a schemaless table, (2) insert rows
where the target field has genuinely mixed `QueryValue` types (e.g. some
`Int`, some `Str`) — i.e. write BEFORE any schema rule is bound, (3) bind a
schema rule declaring that field `TypeTag::Int` (via the existing
`bind_schema` test helper or equivalent `add_schema_rule` call), (4) open a
keyset-requesting cursor ordering by that field, (5) assert
`pinned_mode(...) == PaginationMode::Offset` (proving the P0 is closed — the
gate no longer trusts the schema for pre-existing mixed data), AND drain the
full cursor asserting every row (including the pre-schema mixed-type ones)
is returned exactly once with no loss — this is the concrete proof the fix
works, not just that the gate flag flipped.

## Constraints

- Do NOT implement the harder validated-through-version design — that's an
  explicitly separate, future, larger task.
- Do NOT touch the `Bin`-exclusion finding (F-18/#811) — separate task.
- Do NOT touch anything in `KNOWN_LIMITATIONS.md` outside the one bullet
  identified above.
- Do NOT rename/touch tests unrelated to the schema-typed-scalar gate (e.g.
  the `F64`/`Dec`/`Big`/`List`/`NaN`-exclusion tests are about DIFFERENT
  `TypeTag`s that were EXCLUDED by F-1 regardless of schema — those already
  expect `Offset` and are UNAFFECTED by this change; leave them alone).
- `cargo fmt -p shamir-server` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- cursor_handler
./scripts/test.sh -p shamir-server --full
```
