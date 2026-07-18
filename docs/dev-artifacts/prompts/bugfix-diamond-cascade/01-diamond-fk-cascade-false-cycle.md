# Bug — ON DELETE cascade planner rejects legal diamond FK topologies as false cycles

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## The bug (found by a read-only research audit, confirmed by direct code reading)

`crates/shamir-engine/src/query/batch/fk_actions.rs`:

- `plan_cascade` (lines 90-115) creates ONE `visited: TFxSet<String>` and
  threads it by `&mut` through the entire cascade plan via
  `plan_cascade_recursive` / `plan_cascade_for_ids`.
- `plan_cascade_recursive`'s cycle guard (lines 145-159):
  ```rust
  if !visited.insert(parent_table_ref.table.clone()) {
      // Already cascaded through this table — FK cycle detected.
      return Err(BatchError::query_coded(alias, "fk_cascade_depth", ...));
  }
  ```
- `plan_cascade_for_ids`'s cycle guard (lines 380-391) is the same pattern.
- **Entries are NEVER removed from `visited` when a recursion branch
  returns** — the table name stays in the set for the entire remainder of
  the plan, across ALL sibling branches, not just the branch that inserted
  it.

Because of this, any table reachable via **two distinct FK cascade paths**
(a legal acyclic diamond/DAG, NOT a cycle) trips the guard on the second
path and the recursive call returns `Err(fk_cascade_depth, "cascade cycle
detected...")` — **aborting the entire DELETE**, even though no cycle
actually exists.

### Concrete failure scenario

Tables `B` and `C` both have `ON DELETE CASCADE` FKs referencing `A`.
Table `D` has cascade FKs referencing BOTH `B` and `C` (a diamond:
`A ← B ← D` and `A ← C ← D`). `DELETE FROM A ...` where the delete
cascades rows through both branches:

1. `plan_cascade_recursive(A)` discovers cascade refs to `B` and `C`,
   iterates `by_table` (line 200) — say `B` is processed first.
2. Cascading through `B`'s rows recurses into `plan_cascade_for_ids(D)`
   (line 340) — `visited.insert("D")` succeeds (first time seeing `D`).
3. Back in the outer loop, `C`'s branch is processed — cascading through
   `C`'s rows ALSO recurses into `plan_cascade_for_ids(D)` (same call
   site, line 340, different `child_name`/`parent_table_ref` this time
   for `C`'s children reaching `D`) — `visited.insert("D")` now returns
   `false` (already present from step 2) → the guard fires → the WHOLE
   delete errors with `fk_cascade_depth`, even though `D` is reached via
   two genuinely different, non-cyclic paths.

This is a **false positive that blocks a legal delete** — the opposite
failure mode from a missed-cycle bug, but still a correctness bug: any
schema with a diamond-shaped (or more generally, any DAG-but-not-tree)
cascade topology cannot be deleted through at all.

## The fix

Cycle detection must be **per-path** (i.e. per recursion stack), not
global-across-the-whole-plan:

1. Change the semantics so a table is only considered "in the current
   path" while its own recursive call (and everything it calls
   transitively) is still executing — i.e. **remove the table from
   `visited` when the branch that inserted it returns**, so sibling
   branches don't see it as already-visited. Concretely: after the
   recursive call(s) made from within `plan_cascade_recursive`/
   `plan_cascade_for_ids` for a given table complete (success OR error —
   use an RAII guard or an explicit remove-on-every-exit-path, not just
   the happy path), remove that table's entry from `visited` before
   returning to the caller. The cleanest shape is probably a small RAII
   guard struct (insert on construction, remove on `Drop`) so you don't
   have to manually handle every early-return `?` — check whether the
   existing codebase has a precedent for this pattern (search other
   `TFxSet`/scope-guard usages in `shamir-engine`) before inventing a new
   one; if a "guard that undoes on drop" convention doesn't exist yet,
   define a small private one local to this file, it's a plain sync
   struct with no `.await` held across the guard's lifetime.
2. This alone reopens a SECOND problem: if `D`'s rows are reachable via
   both the `B` branch and the `C` branch, without a table-level
   `visited` block, `D`'s SAME rows could be scheduled for cascade delete
   TWICE (once discovered via `B`, once via `C`) — a double
   `PendingMutation::Delete { table: D, id }` for the same `(table, id)`
   pair. Trace `apply_cascade_plan` (`fk_actions.rs:603-631`): it applies
   `PendingMutation::Delete` unconditionally via `table.delete_tx(id, ...)`
   — a second delete of an already-deleted id will almost certainly
   error ("not found" or similar) inside the transaction, turning the
   false-cycle bug into a different failure (spurious mid-cascade error)
   once the cycle guard itself is fixed. You MUST add **row-level
   deduplication of pending mutations** — e.g. a `TFxSet<(String, RecordId)>`
   (table name + id) tracking mutations already scheduled, checked before
   pushing a new `PendingMutation::Delete`/`SetNull` for the same
   `(table, id)` pair, threaded alongside (or instead of) the per-path
   `visited` set. Decide the cleanest way to thread this — it could live
   in `CascadePlan` itself, or as a sibling parameter next to `mutations`
   in the recursive functions; use your judgement, but the dedup MUST
   cover both `Delete` and `SetNull` mutations, keyed by the target row
   identity, not just by table name.
3. The existing `CASCADE_DEPTH_LIMIT` (checked separately from the
   `visited` set, at the top of both recursive functions) already bounds
   genuinely deep/cyclic recursion — do NOT remove or weaken that check;
   it stays as the depth-based safety valve. The per-path `visited` fix
   is specifically about not flagging DIFFERENT paths through a DAG as a
   cycle, while still catching an ACTUAL cycle (a table reappearing on
   the SAME path, e.g. `A → B → A`).

Read both `plan_cascade_recursive` (lines 117-355) and
`plan_cascade_for_ids` (lines 357-597, read the whole function including
the SetNull id collection and its own grandchild recursion around line
582) in full before writing the fix — both functions have their own
`visited.insert(...)` cycle guard and their own recursive call sites, and
both need the fix applied consistently.

## Tests (find the existing FK cascade/delete test file — likely
## `crates/shamir-engine/src/query/batch/tests/` under a name like
## `fk_actions_tests.rs`/`fk_cascade_tests.rs`; follow its conventions for
## table setup, FK binding, and delete execution)

1. **The diamond topology, closed**: tables `B`, `C` (both `ON DELETE
   CASCADE` → `A`), `D` (cascade FKs → both `B` and `C`). Seed one row in
   `A`, one dependent row each in `B` and `C` (both referencing the same
   `A` row), and one row in `D` referencing BOTH the `B` row and the `C`
   row (i.e. genuinely reachable via both branches). `DELETE FROM A ...`
   must SUCCEED (not error with `fk_cascade_depth`), and the `D` row must
   be deleted **exactly once** (assert it's gone, and — if you can
   observe it — that no double-delete error occurred along the way; a
   clean successful delete of the whole cascade is the main assertion).
2. **Genuine cycle still rejected**: a true cycle (e.g. table `X` with a
   cascade FK to itself creating a self-referential cascade loop, or two
   tables `X`→`Y`→`X` both cascade) must still be correctly rejected with
   `fk_cascade_depth` — do not weaken real cycle detection. (Check
   whether an existing test already covers this; if so, just confirm it
   still passes; if not, add one.)
3. **Regression**: the existing (presumably tree-shaped, non-diamond)
   cascade tests must continue to pass unchanged.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-engine`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the diamond case now succeeds and does not
  double-apply the cascade delete to the shared descendant row, (b) a
  genuine cycle is still correctly rejected, (c) the depth-limit safety
  valve (`CASCADE_DEPTH_LIMIT`) is untouched and still functions as a
  secondary bound.

## Out of scope

- Do NOT touch the ON UPDATE cascade path (`fk_on_update.rs`) — that is a
  separate, already-fixed bug (a different task); this brief is scoped
  entirely to the ON DELETE cascade planner in `fk_actions.rs`.
- Do NOT touch self-referential FK enforcement (a separate, already-known,
  lower-priority gap tracked elsewhere) or FK Int↔F64 type coercion.
- Do NOT touch anything unrelated to the cascade planner's cycle-detection
  and mutation-scheduling logic and its direct test coverage.
