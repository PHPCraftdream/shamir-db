IMPLEMENTATION TASK (TDD, engine — the hard one). Do NOT commit, do NOT push. Rust tests via ./scripts/test.sh (raw cargo test blocked); TS e2e via npx vitest run inside crates/shamir-client-ts. Plan: docs/design/declarative-schema-validators/10-referential-on-delete.md §4.2/4.3 / §5(D.1).

Goal (Phase D.1): enforce `on_delete = Restrict` — reject deleting a parent row that is still referenced by a child FK. Phase D.0 already landed: `FkAction { NoAction, Restrict, Cascade, SetNull }` on `ForeignKeyDto.on_delete` (wire) and `ForeignKeyRef.on_delete` (engine mirror). ONLY implement Restrict in this task (Cascade/SetNull = next task) — for Cascade/SetNull, treat them as "not yet enforced" (skip, like NoAction) and leave a TODO; do NOT half-implement them.

### Architecture decision (already made — FOLLOW IT, it de-risks a lifetime trap):
`TableResolver` is only ever a `&dyn TableResolver` borrow — there is NO `Arc<dyn TableResolver>`. The implicit/autocommit delete path runs inside `run_implicit_batch_tx`, whose `for<'t>` HRTB closure can borrow ONLY the `tx` — you CANNOT capture `self.resolver` into it. Do NOT fight this.

**Implement the Restrict gate at the `BatchOp::Delete` arm in
`crates/shamir-engine/src/query/batch/query_runner.rs` (around line 540-590), BEFORE dispatching to `execute_delete_tx`** — where `self.resolver` (`&'a dyn TableResolver`) is freely available for BOTH the tx-mode branch (line ~550) and the implicit branch (line ~575). Steps:
1. Build a "reverse-FK map": which tables in this repo have an FK rule (`ForeignKeyRef`) whose `ref_table` == the table being deleted from AND `on_delete == Restrict`. Source: each table's schema rules (the `SchemaValidator` / `FieldRule.constraints.foreign_key`). Find how to enumerate tables in a repo + read their schema rules (grep `validator_bindings` / `schema` / `list_tables` / the repo's table registry). If no such referencing table → no gate, proceed as today.
2. Determine the to-be-deleted rows' referenced-field values: scan the rows matching `op.where_clause` (reuse the read path / a lightweight scan via `self.resolver`-resolved table), extract the value of each child's `ref_field` for each row. (MVP: a pre-scan here is acceptable — delete is not a hot path and Restrict tables are opt-in. Document the TOCTOU caveat: the check is not in the same atomic snapshot as the delete; tightening to in-tx requires an Arc-resolver refactor = a future task.)
3. For each referencing (child_table, child_field) with Restrict, ask: does any child row have child_field == one of the parent values? Use the existing read/index path (mirror `ValidatorDb::exists_in` semantics in validator_db.rs: index lookup if covered, else scan). If ANY child references a row being deleted → return `BatchError::query_coded(alias, "fk_restrict", "...")` and DO NOT delete.
4. If the gate passes (or no Restrict referencers) → proceed with the existing delete dispatch unchanged.

Keep the gate logic in a focused helper (new file under query/batch/ or table/, one-primary-export per CLAUDE.md), called from the Delete arm.

### Tests (TDD)
- 🔴 Engine unit (crates/shamir-engine, tests/ layout): create parent table + child table with `foreign_key(parent, id).on_delete(Restrict)`; insert parent row + a child referencing it; delete parent → expect rejection (fk_restrict error), parent still present. Delete the child first → then delete parent → succeeds.
- 🔴 Edge: NoAction FK → delete parent succeeds even with a child (no gate). Parent with NO referencers → deletes fine.
- 🟢 implement until green.
- TS e2e (crates/shamir-client-ts/src/__tests__/e2e-fk-ondelete.test.ts NEW, use the harness): createTable parent+child, schema with `.foreignKey('parent','id',{onDelete:'restrict'})` on child + required indexes; insert parent+child; delete parent via builder → expect error containing fk_restrict; delete child → delete parent OK.
- Run: ./scripts/test.sh -p shamir-engine ; then rebuild server `cargo build --release -p shamir-server` (CARGO_TARGET_DIR is already set in env to D:\dev\rust\.cargo-target — the e2e harness resolves the binary there); then npx vitest run src/__tests__/e2e-fk-ondelete.test.ts inside crates/shamir-client-ts.

End with a final message: where you put the gate + helper, how you enumerate referencing tables, the TOCTOU caveat, and pass counts.

RATE-LIMIT: do this YOURSELF in a single agent — NO sub-agents. Use grep/view directly. If you get stuck on enumerating referencing tables or the reverse-FK lookup, say so explicitly in your final message rather than guessing.
