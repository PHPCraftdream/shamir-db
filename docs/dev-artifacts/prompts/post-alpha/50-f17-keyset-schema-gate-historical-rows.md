# Brief for F-17 (#810, P0) — keyset schema-typed gate doesn't prove historical-row homogeneity

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context: a confirmed P0, found independently three times

F-1 (#792) added a gate so a keyset cursor's `ORDER BY` column is only
eligible for `PaginationMode::Keyset` when the table has a bound schema
rule declaring that column a non-container scalar `TypeTag`
(`Int`/`Bool`/`String`/`Bin`) — the doc comment claims this proves the
column is "provably homogeneous by construction — mixed-type is
impossible."

Three independent post-wave reviews of Wave F (two agent-driven, one
deeper static audit — see
`docs/dev-artifacts/research/2026-07-26-wave-f-consolidated-synthesis/SYNTHESIS.md`
for the full cross-reference) converged on the same finding, and the
orchestrator has personally verified it by reading the code:
`add_schema_rule`/`set_table_schema`
(`crates/shamir-db/src/shamir_db/execute/admin_schema.rs`) validate the new
rule's shape (FK/unique index requirements, nested-path/transform/cascade
rejections) but **never scan or validate the table's EXISTING rows**
against the new rule. So the "provably homogeneous" claim only holds for
rows written AFTER the schema rule was bound — not for any row that
existed in the table BEFORE that moment.

**Confirmed repro:**
1. A schemaless table already has mixed-type data in some field (e.g.
   `score: Int` on some rows, `score: Str` on others — this is legal for a
   schemaless document table).
2. An operator later declares a schema: `score: Int`.
3. `order_by_column_is_schema_typed_scalar`
   (`crates/shamir-server/src/db_handler/cursor_handlers.rs:475-510`) now
   returns `true` for `score` — the metadata says "Int, and only Int."
4. A keyset cursor `ORDER BY score` enters `PaginationMode::Keyset`
   (`cursor_handlers.rs:1237-1251`).
5. The keyset boundary filter's `compare_values` (`shamir-engine`'s
   `query/filter/resolve.rs`) has no cross-type comparison arm — an old
   `Str` row against an `Int` seek key falls through to `_ => None`,
   meaning the boundary filter is `false` for that row.
6. That pre-existing `Str` row silently vanishes from later pages — the
   EXACT bug class F-1 (#792) was written to close, recurring through an
   unenforced assumption.

## Design (investigated, decided — implement this, don't re-derive from scratch)

**Do NOT disable keyset pagination entirely.** That was one option raised
during triage (the "safest but costliest" alpha fix suggested by the third
review), but it guts a real, working, tested performance feature (see
`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`'s
`keyset_tie_run_*`/`keyset_heavy_duplication_*`/`keyset_ceiling_*` tests —
all of them currently reach genuine `PaginationMode::Keyset` via
`build_handler_with_scores`'s pattern of `bind_schema(...)` BEFORE seeding
any rows) for a problem that has a cheap, precise fix.

**The fix: track whether a field's schema rule was bound while the table
was PROVABLY EMPTY, and only trust the keyset gate for fields where that's
true.**

`TableManager::count()`
(`crates/shamir-engine/src/table/table_manager_crud.rs:470-472`) is **O(1)**
(a stored counter, not a scan) — `table.count().await? == 0` at the exact
moment a schema rule for a field is bound is a cheap, precise proof: if the
table had zero rows at that instant, NO existing row can violate the new
rule (schema enforcement covers 100% of the table's history from that
point forward), so the column genuinely is homogeneous by construction —
the ORIGINAL claim, just now actually verified instead of assumed. If the
table was non-empty at bind time, this specific field's homogeneity is NOT
proven — the gate must treat it exactly like a schemaless column
(ineligible, falls back to `PaginationMode::Offset`, the existing safe
default) — this is the sole and complete fix; do not attempt a full
retroactive data-scan/backfill-validation feature, that's explicitly out of
scope for this task (bigger design space, tracked separately if ever
needed).

### Where to persist the "keyset_safe" proof

`FieldRuleDto` (`crates/shamir-query-types/src/admin/types/schema_ops.rs:35-44`)
is BOTH the wire request shape (what a client sends in `AddSchemaRule`/
`SetTableSchema`) AND the catalogue-persisted shape (loaded back via
`dto_list_from_catalogue`/serialised via `serialise_rules` in
`admin_schema.rs`, e.g. lines ~495, ~551, ~641, ~674). Add a new field to
it, e.g.:

```rust
/// Server-computed proof that this rule was bound while the table had
/// zero rows — i.e. every row the table has ever held was validated
/// against this rule from the start, making the column provably
/// homogeneous for the keyset-cursor safety gate
/// (`order_by_column_is_schema_typed_scalar`,
/// `crates/shamir-server/src/db_handler/cursor_handlers.rs`).
///
/// **SERVER-COMPUTED ONLY.** A client MAY send this field (it's part of
/// the same wire DTO used for requests), but the server MUST ignore
/// whatever the client sent and overwrite it with a freshly-computed
/// `table.count().await? == 0` check at the exact moment the rule is
/// bound (`handle_add_schema_rule`/`handle_set_table_schema`) — never
/// trust client input for a correctness-relevant safety flag.
#[serde(default, skip_serializing_if = "std::ops::Not::not")]
pub keyset_safe: bool,
```

(Adjust the exact serde attribute/placement to fit the file's existing
style — read the surrounding fields first.) Then, in
`admin_schema.rs`'s `handle_add_schema_rule` (~457) and
`handle_set_table_schema` (the sibling handler earlier in the same file —
read it first, don't guess its exact line range), right before persisting
the rule into `rec` via `SCHEMA_FIELD`: for each NEW or CHANGED rule in the
batch being persisted, compute `let keyset_safe = table.count().await? ==
0;` (using the SAME `table` handle already resolved for the FK/unique
validation calls) and stamp it onto that rule's `FieldRuleDto` before
calling `serialise_rules`. Rules that are UNCHANGED from a previous
`add_schema_rule` call (upsert-by-path replacing an identical rule) should
preserve their PREVIOUSLY recorded `keyset_safe` value, not recompute it —
only a genuinely NEW or type-changed rule needs a fresh emptiness check
(investigate `alter`/`drop` rule handlers too, apply the same principle:
only compute fresh proof for rules that are newly bound or whose `TypeTag`
changed).

### Consuming the proof

`order_by_column_is_schema_typed_scalar`
(`cursor_handlers.rs:475-510`) currently returns `true` based solely on
`matched_ty` being one of the accepted `TypeTag`s. Thread the persisted
`keyset_safe` flag through the same lookup path (`table.validator_bindings()`
→ `registry.get_by_id` → `validator.as_schema_rules()` → matching `rule.path
== field`) and require BOTH: `matched_ty` is an accepted `TypeTag` AND the
matched rule's `keyset_safe == true`. If either fails (unset/false, or
disagreement across multiple bound validators — keep the existing
conservative "disagree → not schema-typed" behavior), the column is
ineligible, same as today's "no schema bound at all" case.

You will likely need to surface `keyset_safe` on the engine-side
`FieldRule` struct too (`crates/shamir-engine/src/validator/schema/field_rule.rs:27-33`,
currently `path`/`ty`/`constraints` only) since that's what
`RecordValidator::as_schema_rules()` hands back to cursor_handlers.rs — add
the field there, threading it through wherever `FieldRule` is constructed
from `FieldRuleDto` (find that conversion site; it's the natural place to
carry the flag across the DTO→engine-struct boundary).

### Backward compatibility (critical safety invariant)

A catalogue record persisted by any code BEFORE this fix has no
`keyset_safe` field recorded at all in its serialized rules. The `serde`
default for the new field on `FieldRuleDto` MUST be `false` (unproven) —
**never `true`** — so that on load, every pre-existing schema rule is
treated as NOT proven keyset-safe until server-side logic re-establishes
that proof (which won't happen automatically for existing rules — this is
an accepted, honest consequence: tables that already had a schema bound
before this fix ships will fall back to `Offset` mode until their schema is
re-declared, which is the CORRECT safe behavior, not a bug to route
around). Do not add any migration/backfill logic to retroactively mark old
rules as `keyset_safe: true` — that would silently reintroduce exactly the
bug this task closes.

## Tests

1. **No regression for the existing (correct, common) schema-first
   workflow**: every existing test in `cursor_handler_tests.rs` that binds
   a schema via `bind_schema(...)` BEFORE seeding rows (i.e. essentially
   all of the current `keyset_*`/`schema_typed_*`/`null_probe_*` tests —
   grep the file for `bind_schema` call sites and confirm each one precedes
   its row-seeding step) must continue to reach genuine
   `PaginationMode::Keyset` UNCHANGED. Run the full file before and after
   your change and diff the pass/fail list — it must be identical.
2. **New regression test for the confirmed repro**: seed a table with
   ALREADY-MIXED-TYPE data in a field (some rows `Int`, some `Str` — no
   schema bound yet), THEN bind a schema declaring that field `Int` via
   `add_schema_rule`/`set_table_schema`, THEN open a keyset-requesting
   cursor `ORDER BY` that field. Assert `pinned_mode(...) ==
   PaginationMode::Offset` (NOT Keyset), and drain the full cursor across
   multiple pages asserting every row (both the pre-existing mixed-type
   ones and any new ones) is returned exactly once — the actual
   correctness guarantee this fix buys.
3. **`keyset_safe` persistence round-trip**: bind a schema on an empty
   table (expect `keyset_safe: true` recorded), reload/redescribe the table
   (or restart the in-memory `ShamirDb` instance the test helpers use) and
   confirm the flag survives and the gate still returns eligible.
4. **Upsert-by-path preserves prior proof**: bind a rule on an empty table
   (proven safe), then call `add_schema_rule` again with an IDENTICAL rule
   for the same path — confirm `keyset_safe` stays `true` (not recomputed
   against a now-non-empty table, which would incorrectly flip it to
   `false` and silently degrade a working, correct cursor to Offset for no
   reason).
5. Update the doc comment on `order_by_column_is_schema_typed_scalar` (and
   its callers) plus `docs/guide-docs/KNOWN_LIMITATIONS.md` §6's "Mixed
   `QueryValue` type in one `ORDER BY` column" bullet to precisely describe
   the REAL closed fix: "CLOSED for schema-typed scalar columns whose rule
   was bound while the table was empty (verified via a persisted
   `table.count() == 0`-at-bind-time proof); a schema declared onto an
   already-populated table's column falls back to `Offset`, same as a
   schemaless column, until that rule is proven safe some other way."

## Constraints

- Do NOT implement a full retroactive backfill/validation-scan feature —
  out of scope, tracked separately if ever needed.
- Do NOT disable keyset pagination entirely — the existing, tested,
  schema-bound-before-write workflow must keep working exactly as today.
- Do NOT trust client-supplied `keyset_safe` input — always
  server-recompute at bind time for new/changed rules.
- Do NOT retroactively mark pre-existing (already-persisted) schema rules
  as `keyset_safe: true` — the safe default for anything without a fresh
  proof is `false`/ineligible.
- `TypeTag::Bin`'s inclusion/exclusion in the accepted set is a SEPARATE,
  already-tracked task (#811/F-18) — do not fold that change into this
  brief; leave `Bin` handling exactly as it is today (this task only adds
  the `keyset_safe` gate on top of the existing `TypeTag` check).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -p shamir-server -p shamir-engine -p shamir-query-types`
  and `cargo clippy` on the same crates with `--all-targets -- -D warnings`
  must be clean.
- Surgical diff — no incidental refactors beyond what this task needs.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -p shamir-server -p shamir-engine -p shamir-query-types -- --check
cargo clippy -p shamir-db -p shamir-server -p shamir-engine -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- cursor_handler
./scripts/test.sh -p shamir-db -- schema
./scripts/test.sh @oracle
```
