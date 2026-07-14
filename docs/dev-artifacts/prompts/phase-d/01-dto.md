IMPLEMENTATION TASK (TDD). Do NOT commit, do NOT push. Rust tests via ./scripts/test.sh (raw cargo test blocked); TS via npx vitest run inside crates/shamir-client-ts.

Goal (Phase D.0): add the `on_delete` referential-action field to the foreign-key DTO + engine mirror + Rust/TS builders + serde tests. NO engine delete-logic in this task (that is D.1). Plan: docs/dev-artifacts/design/declarative-schema-validators/10-referential-on-delete.md §4.1 / §5(D.0).

### The decision (already made — bake it in EXACTLY):
- New enum `FkAction { NoAction, Restrict, Cascade, SetNull }`.
- **serde / `Default` for `FkAction` = `NoAction`** — conservative WIRE default so EXISTING persisted schemas (stored without `on_delete`) deserialize to NoAction and DO NOT change delete behavior on reload. This is a hard backward-compat requirement.
- **Builder default = `Restrict`** — a NEW foreign-key declared via the builder defaults to Restrict (safe-by-default), with explicit opt-out. The builder sets Restrict EXPLICITLY; it must NOT rely on `FkAction::default()`.

### Part 1 — wire DTO (crates/shamir-query-types/src/admin/types/schema_ops.rs)
- `ForeignKeyDto { ref_table: String, ref_field: String }` → add `on_delete: FkAction`.
- Define `FkAction` (in this crate; one-primary-export-per-file per CLAUDE.md — likely a new file `fk_action.rs` in the same module, re-exported via mod.rs). `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]`, `#[serde(rename_all = "snake_case")]` or match the crate's enum-tag convention (check a sibling enum), `#[default] NoAction`.
- On the field: `#[serde(default, skip_serializing_if = "FkAction::is_no_action")]` (add `is_no_action(&self)->bool`) so existing wire bytes roundtrip unchanged and NoAction is omitted.

### Part 2 — engine mirror (crates/shamir-engine/src/validator/schema/foreign_key.rs)
- `ForeignKeyRef { ref_table, ref_field }` → add `on_delete: FkAction` (mirror enum — either re-use the query-types `FkAction` if engine already depends on it, or a parallel engine enum like other mirrors here; check how `ForeignKeyRef` is built from `ForeignKeyDto` and mirror that conversion). Keep `ForeignKeyRef::new(table, field)` defaulting `on_delete` to `NoAction` (backward compat for existing callers); add a constructor/param to set the action.
- Wire up the DTO→engine conversion for the new field (find where ForeignKeyDto becomes ForeignKeyRef — grep `ForeignKeyRef::new` / `foreign_key`).

### Part 3 — builders
- Rust (crates/shamir-query-builder/src/ddl/schema.rs FieldBuilder): the existing `.foreign_key(table, field)` must now default the action to **Restrict**. Add an action-taking form: either `.foreign_key(table, field)` (defaults Restrict) + `.on_delete(FkAction)` chained, OR `.foreign_key_on_delete(table, field, action)`. Pick the ergonomic one; state it.
- TS (crates/shamir-client-ts/src/core/builders/ddl.ts + types): `.foreignKey(table, field, { onDelete })` where onDelete defaults to `'restrict'`. Add the wire type for on_delete (snake_case action strings: `'no_action'|'restrict'|'cascade'|'set_null'`). Check the existing foreignKey builder + the ConstraintsDto TS type.

### Tests (TDD)
- 🔴 serde round-trip: `ForeignKeyDto` with each FkAction value → exact wire shape; AND a legacy `ForeignKeyDto` WITHOUT `on_delete` deserializes to `NoAction` (the backward-compat invariant — this is the critical test).
- Rust builder unit: `.foreign_key("parent","id")` → constraint has `on_delete == Restrict` (builder default); explicit Cascade form works.
- TS unit (ddl.test.ts): `.foreignKey('parent','id')` emits `on_delete: 'restrict'`; `{onDelete:'cascade'}` emits cascade.
- Run: ./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-query-builder ; then npx vitest run src/core/builders/__tests__/ddl.test.ts

Keep diffs surgical, imports at top, one-primary-export-per-file. End with a final message: chosen builder signature, the serde-default-NoAction vs builder-default-Restrict split confirmation, and pass counts.

RATE-LIMIT: do this YOURSELF in a single agent — NO sub-agents.
