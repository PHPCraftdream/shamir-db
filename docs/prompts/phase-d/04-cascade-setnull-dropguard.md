# Phase D.2/D.3 — CASCADE + SET NULL + drop-guard — implementation brief

IMPLEMENTATION TASK (TDD, Rust engine). Repo: D:\dev\rust\shamir-db.

⛔ ABSOLUTELY FORBIDDEN: `git reset` / `checkout` / `clean` / `stash` / `restore`
/ `rm`, or ANY git-mutating command. Only edit files; the orchestrator commits.
(An agent ran `git reset --hard` and destroyed hours of work — never touch git.)
Do NOT commit, do NOT push. Do NOT spawn sub-agents. Tests ONLY via
./scripts/test.sh (raw cargo test is blocked). Read FULL test output.

Design doc: docs/design/declarative-schema-validators/10-referential-on-delete.md
(§5 D.2/D.3). Phase D.1 (RESTRICT) already landed + COMMITTED:
- `FkAction { NoAction, Restrict, Cascade, SetNull }` on `ForeignKeyRef.on_delete`.
- `crates/shamir-engine/src/query/batch/fk_restrict.rs` — the reverse-FK gate
  (`check_fk_restrict`) called at the `BatchOp::Delete` arm in query_runner.rs,
  using `self.resolver`. It discovers referencing (child_table, child_field) refs
  via `resolve_repo → list_table_names → child.collect_fk_refs()` filtered to
  `ref_table==parent && on_delete==Restrict`. READ this file first — D.2/D.3
  extend the SAME discovery + gate machinery to the other actions.

GOAL: implement the remaining referential actions. Currently Cascade/SetNull are
treated like NoAction (skipped, with a `// TODO Phase D.2`). Replace that.

### D.2 — CASCADE + SET NULL
Extend the gate (fk_restrict.rs, or a sibling module) so the discovery also
collects refs with `on_delete == Cascade` and `== SetNull`, and ACTS on them at
the Delete arm (before/around the parent delete), within the SAME implicit/tx
batch so it commits atomically with the parent delete:
- **Cascade:** for each referencing child row (child_field == a parent value
  being deleted), DELETE the child row. Recurse (a cascaded child may itself be
  a parent of grandchildren) with a DEPTH GUARD to prevent infinite loops on FK
  cycles — reuse the sub-batch depth guard if one exists, else a small explicit
  limit; on exceeding it, reject with a coded error (`fk_cascade_depth`).
- **SetNull:** for each referencing child row, UPDATE it setting child_field to
  Null. Requires the child field to be nullable — enforce a bind-time check:
  binding/declaring a SetNull FK on a non-nullable field is a DDL-time error
  (`set_null_requires_nullable`). (If bind-time is hard to reach, document why
  and check at action time instead, returning the same coded error.)
Mirror the existing child-row discovery/probe (index-first, fallback scan) from
fk_restrict.rs. Keep the TOCTOU caveat note consistent with D.1.

### D.3 — drop-guard symmetry
- `DropTable`: refuse (coded error `drop_refused_fk`) if another table has a live
  FK pointing at it — UNLESS a cascade is requested. Find the DropTable handling
  (grep `BatchOp::DropTable` / `drop_table`) and add the reverse-FK check
  (reuse the discovery from D.1 — any referencing FK, not just Restrict).
- `DropFunction`: refuse (`drop_refused_bound`) if the function is bound as a
  validator. Find DropFunction handling; check the validator bindings registry.

### TESTS (TDD — engine tests/ layout)
- 🔴 Cascade: parent + child(Cascade FK); delete parent → child is ALSO deleted;
  parent gone. Chain A→B→C cascade: deleting A removes B and C. Cycle A→B→A →
  depth-guard error, no partial corruption.
- 🔴 SetNull: parent + child(SetNull FK, nullable field); delete parent → child
  survives with child_field == Null. SetNull on non-nullable field → bind/DDL
  error.
- 🔴 drop-guard: DropTable on a referenced table → refused; DropFunction on a
  bound validator → refused.
- 🟢 implement until green. Run: ./scripts/test.sh -p shamir-engine
- TS e2e: extend crates/shamir-client-ts/src/__tests__/e2e-fk-ondelete.test.ts
  with cascade + set-null cases (do NOT run — orchestrator runs e2e after server
  rebuild).

When done: `cargo fmt -p shamir-engine -- --check` (fix with cargo fmt) +
`cargo clippy -p shamir-engine` (fix warnings you add).

Final message: files changed, how cascade recursion + depth guard work, how
SetNull nullable is enforced, where the drop-guards live, test pass counts,
fmt/clippy status. If stuck, STOP and report the exact blocker — do not fake a
test. NEVER touch git.
