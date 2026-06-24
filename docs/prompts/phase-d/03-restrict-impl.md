# Phase D.1 — RESTRICT (reverse-FK gate) — implementation brief

IMPLEMENTATION TASK (TDD, Rust engine — the hard one). Repo: D:\dev\rust\shamir-db.

⛔ ABSOLUTELY FORBIDDEN: `git reset` / `checkout` / `clean` / `stash` / `restore`
/ `rm`, or ANY git command that mutates the working tree or index. Only edit
files; the orchestrator commits. (An agent ran `git reset --hard` and destroyed
hours of work — never touch git state. If you think you need to "clean up", you
do NOT.) Do NOT commit, do NOT push. Do NOT spawn sub-agents — work directly.

Tests ONLY via ./scripts/test.sh (raw `cargo test` is blocked by a perimeter
guard). Read the FULL output; if cutting noise, write to a file and grep the FILE.

Design doc: docs/design/declarative-schema-validators/10-referential-on-delete.md
A WIP gate from a prior (killed) attempt is at
.recovery-backup/fk_restrict_gate.rs — use it ONLY as a reference; write your own
clean implementation.

GOAL: enforce `on_delete = Restrict` — reject deleting a parent row still
referenced by a child FK. Already landed + COMMITTED: `FkAction { NoAction,
Restrict, Cascade, SetNull }` on `ForeignKeyDto.on_delete` (query-types) + the
builder default Restrict + catalogue round-trip (admin_schema). Implement ONLY
Restrict here; for Cascade/SetNull leave a `// TODO Phase D.2` and behave like
NoAction (skip).

ARCHITECTURE DECISION (made — FOLLOW IT, it avoids a lifetime trap):
`TableResolver` is ONLY ever a `&dyn TableResolver` borrow — there is NO
`Arc<dyn TableResolver>`. The implicit/autocommit delete path runs inside
`repo.run_implicit_batch_tx(...)` whose `for<'t>` HRTB closure can borrow ONLY
`tx` — you CANNOT capture `self.resolver` into it. Do NOT fight this.

Implement the Restrict gate at the `BatchOp::Delete` arm in
crates/shamir-engine/src/query/batch/query_runner.rs (around lines 540-590),
BEFORE dispatching to `execute_delete_tx`, where `self.resolver`
(`&'a dyn TableResolver`) is freely available for BOTH the tx-mode branch
(~line 550) and the implicit branch (~line 575). Algorithm:
1. Reverse-FK discovery: find every table in this repo whose schema has a
   `FieldRule.constraints.foreign_key` with `ref_table == <table being deleted
   from>` AND `on_delete == Restrict`. Investigate how to enumerate tables in a
   repo and read each table's schema rules (grep `validator_bindings`,
   `SchemaValidator`, the repo/table registry, `get_table_schema`). If none →
   no gate, proceed unchanged.
2. Compute the referenced values of the to-be-deleted rows: scan rows matching
   `op.where_clause` and extract each child's `ref_field` value (usually the
   parent key). A pre-scan here is acceptable for the MVP (delete is not a hot
   path; Restrict tables are opt-in). DOCUMENT the TOCTOU caveat inline: the
   check is not in the same atomic snapshot as the delete; tightening to in-tx
   needs an Arc-resolver refactor (future task).
3. For each referencing (child_table, child_field) with Restrict: does any
   child row have child_field == one of the parent values? Mirror
   `ValidatorDb::exists_in` (crates/shamir-engine/src/validator/validator_db.rs):
   index lookup if covered, else `list_stream` scan. Resolve child tables via
   `self.resolver`.
4. If ANY child references a row being deleted → return
   `BatchError::query_coded(alias, "fk_restrict", "<msg>")`, do NOT delete.
   Else proceed with the existing delete dispatch unchanged.
Put the gate in a focused helper (new file under query/batch/, ONE primary
export per CLAUDE.md), called from the Delete arm. Imports at top.

TESTS (TDD — engine tests/ layout: tests/ dir + mod.rs manifest):
- 🔴 parent table + child table; child schema has `foreign_key(parent,id)
  .on_delete(Restrict)`; insert parent + a child referencing it; delete parent
  → reject (error code/text contains fk_restrict), parent still present; delete
  child first, THEN delete parent → succeeds.
- 🔴 edges: NoAction FK → deleting parent succeeds even with a child; a parent
  with no referencers deletes fine.
- 🟢 implement until green.
- Run: ./scripts/test.sh -p shamir-engine
- TS e2e: create crates/shamir-client-ts/src/__tests__/e2e-fk-ondelete.test.ts
  (use the harness e2e-harness.ts): parent+child tables, child schema
  `.foreignKey('parent','id',{onDelete:'restrict'})` + required indexes, insert
  parent+child, delete parent via the builder → expect error containing
  fk_restrict; delete child → delete parent OK. (Do NOT run it — the orchestrator
  rebuilds the server and runs all e2e together.)

When done run `cargo fmt -p shamir-engine -- --check` (fix with `cargo fmt -p
shamir-engine`) and `cargo clippy -p shamir-engine` (fix warnings you add).

Final message: files added/changed, HOW you enumerated referencing tables, the
TOCTOU caveat wording, engine test pass counts, fmt/clippy status. If genuinely
stuck on enumerating referencing tables, STOP and report exactly what blocked
you (file/line) — do NOT guess or fake a passing test. NEVER touch git.
