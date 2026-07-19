# Cleanup tail B — self-referential FK enforcement + FK Int↔F64 coercion

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

This brief covers TWO findings (10, 11) from the release audit
(`docs/dev-artifacts/research/2026-07-17-release-audit/04-logical-correctness-bugs.md`).
Finding 10 (self-referential FKs) is investigated in unusual depth below —
**read that reasoning carefully before writing any code**, because the
correct fix is DIFFERENT depending on which FK action and which file, and a
naive "just remove the skip everywhere" change will break something (see
"Why this is NOT one uniform fix" below).

---

## Fix 1 (Finding 11) — FK child matching drops Int↔F64 coercion

### The bug

Two files each define an IDENTICAL private pair of functions:

- `crates/shamir-engine/src/query/batch/fk_actions.rs:959-991`
  (`record_field_matches_qv` / `scalar_ref_matches_qv`)
- `crates/shamir-engine/src/query/batch/fk_on_update.rs:794-826`
  (same two function names, same bodies)

`scalar_ref_matches_qv` does exact same-variant matching only:

```rust
fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    match (actual, value) {
        (ScalarRef::Null, QueryValue::Null) => true,
        (ScalarRef::Bool(a), QueryValue::Bool(b)) => a == b,
        (ScalarRef::Int(a), QueryValue::Int(b)) => a == b,
        (ScalarRef::F64(a), QueryValue::F64(b)) => a == b,
        (ScalarRef::Str(a), QueryValue::Str(b)) => *a == b.as_str(),
        (ScalarRef::Bin(a), QueryValue::Bin(b)) => *a == b.as_slice(),
        _ => false,
    }
}
```

If the parent key is stored as `Int(5)` and the child FK field is stored as
`F64(5.0)` (or vice versa — e.g. a client that sends all numbers as
floats), this returns `false` — the child row is invisible to cascade/
setnull/restrict scans, so it silently survives a parent delete with a
dangling reference (or blocks a RESTRICT check that should have fired).
Every other comparison layer in this engine (`scalar_ref_cmp_qv`,
`compare_values`, the `set_contains_coercing*` family from a prior stage of
this same cleanup campaign) already treats `Int(5)`/`F64(5.0)` as equal —
FK enforcement is the one place that's stricter than query semantics.

### The fix

`shamir_types::record_view::scalar_ref_cmp_qv` (already imported in both
files — check the existing `use` block) already implements the correct
cross-type comparison (`ScalarRef::Int(a)` vs `QueryValue::F64(b)` compares
`(a as f64).partial_cmp(b)`, and the reverse). Replace BOTH copies of
`scalar_ref_matches_qv`'s body with a delegation to it:

```rust
fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    scalar_ref_cmp_qv(*actual, value) == Some(std::cmp::Ordering::Equal)
}
```

(`scalar_ref_cmp_qv` takes `ScalarRef<'_>` by value, not `&ScalarRef<'_>` —
dereference `actual`.) Do this in BOTH files — they are independent
copies, not shared code; fixing one does not fix the other.

### Tests

1. Parent key `Int(5)`, child FK field `F64(5.0)` — a `Cascade` delete on
   the parent must actually cascade to (delete) the child row.
2. Same in reverse (parent `F64`, child `Int`).
3. Same coercion must work for `RESTRICT` (a float-typed child FK value
   referencing an int-typed parent key must correctly block the delete)
   and `SET NULL`.
4. Same coercion must work on the `fk_on_update.rs` path (an UPDATE that
   re-keys a parent whose old/new ref values are compared against
   float-typed child FK fields).
5. Regression: exact-type matches (Int↔Int, Str↔Str, etc.) continue to
   work exactly as before.

---

## Fix 2 (Finding 10) — self-referential FKs silently unenforced

### Why this is NOT one uniform fix

There are THREE independent "skip the parent table itself" sites, and they
have genuinely different safety properties. Read all three investigations
below before changing anything — the naive fix ("just delete the `if name
== &parent_table_ref.table { continue; }` line everywhere") is UNSAFE for
one of the three and will produce confusing false "cycle" errors on
ordinary, shallow self-referential deletes.

**Site A — `crates/shamir-engine/src/query/batch/fk_restrict.rs`, inside
`discover_restrict_refs` (~line 158-162):** `check_fk_restrict` is a FLAT,
non-recursive scan — it discovers child tables, then does ONE existence
check per parent value. There is no cycle-guard, no depth limit, nothing
recursive here at all. Self-referential RESTRICT is 100% safe to enable:
removing the skip means table X is now also scanned as its own potential
child, so `employees.manager_id REFERENCES employees.id ON DELETE RESTRICT`
correctly blocks deleting a manager who still has subordinates. **Fix: just
delete the skip.**

**Site B — `crates/shamir-engine/src/query/batch/fk_on_update.rs`, inside
the ref-discovery function (~line 643-649, `discover_on_update_refs` or
similarly named — read the function to confirm):** Read this file's own
module doc comment (~lines 26-32) FIRST — it explicitly states the entire
ON UPDATE mechanism is **single-level, MVP-scoped, by design**, for ALL
three actions (Restrict/Cascade/SetNull) and for EVERY child table, not
just self-referential ones ("A re-keyed child row is not itself re-scanned
for grandchildren... avoids FK-cycle recursion entirely"). Since there is
NO recursion anywhere in this file regardless of self-reference, enabling
self-referential Restrict/Cascade/SetNull here is safe **in principle** —
but the module comment also flags a "ping-pong" concern for
"self/mutual FK cycles" that you must independently verify does NOT apply
before shipping this: specifically, check whether the child-row scan for a
self-referential FK could match one of the SAME rows that are being updated
by the parent operation itself (i.e., a row is both "one of the parent rows
whose ref_field is changing" and "a child row whose FK references some
OTHER parent row's old ref_field value" — these can be different rows even
within the same table, so this should be fine, but write a targeted test
for it, see below). **Fix: delete the skip, but add the self-loop edge-case
test below before declaring this safe.**

**Site C — `crates/shamir-engine/src/query/batch/fk_actions.rs`, inside
`discover_action_refs` (~line 858-863):** This is the DELETE-path Cascade/
SetNull discovery, and unlike sites A/B, `plan_cascade_recursive` /
`plan_cascade_for_ids` in this SAME file recurse for `Cascade` (NOT for
`SetNull` — SetNull mutations are recorded once and never trigger further
recursion, see ~line 428 "Recurse for grandchildren (Cascade only)").  This
file also has the `CascadePathGuard` (added in an earlier stage of this
campaign, task 1c) — a table-NAME-based per-path cycle guard: entering
table X a second time while X is still an ancestor on the current
recursion stack is treated as a genuine A→B→A cycle and rejected with
`fk_cascade_depth`.

Trace through what happens if you naively remove the skip for BOTH actions
here: deleting manager M (root call enters "employees" into `visited`).
Self-ref discovery now finds "employees" as a child of "employees" with a
`Cascade` action. If M has ANY direct subordinates, `cascade_ids` is
non-empty, and the code recurses into `plan_cascade_for_ids` for
child_name="employees" — which immediately calls
`CascadePathGuard::enter(visited, "employees", ...)` and finds "employees"
ALREADY in `visited` (the root call's guard is still held, we're inside its
stack frame) → **immediately returns `Err(fk_cascade_depth)`**, even though
this is just the most ordinary, shallow case (a manager with direct
reports, zero further recursion needed). This is a FALSE "cycle" rejection,
not a real one — the guard's table-name-based model cannot currently
distinguish "genuine external cross-table cycle" from "the same table
recursing into itself, which is what self-referential cascade fundamentally
requires."

Properly supporting self-referential CASCADE at arbitrary depth would
require redesigning the cycle-guard to track something finer-grained than
table names (e.g. per-row visited-ID sets, or treating "the table the
current recursion originated from" specially) — this is a real feature,
not a small cleanup fix, and is explicitly **OUT OF SCOPE** for this brief.

**Fix for Site C — split by action:**
- **SetNull**: safe to enable (never recurses) — but `discover_action_refs`
  currently returns a SINGLE list covering both Cascade and SetNull refs,
  gated by ONE skip check. You need to stop skipping self-referential
  SetNull refs while CONTINUING to skip (or otherwise excluding) self-
  referential Cascade refs specifically. Read `discover_action_refs`'s
  full body and `DiscoveredRef`'s shape before deciding the cleanest way to
  do this — e.g. keep the self-ref skip for `fk.on_delete == FkAction::Cascade`
  only, while allowing it through for `fk.on_delete == FkAction::SetNull`.
- **Cascade**: do NOT attempt to enable this at runtime — the architecture
  genuinely cannot support it safely today (see trace above). Instead,
  reject a self-referential `ON DELETE CASCADE` FK declaration at DDL time
  with a clear coded error, mirroring the EXACT pattern already established
  in this campaign for an analogous "convert silent-wrong into honest
  explicit error" case: `crates/shamir-db/src/shamir_db/execute/admin_schema.rs`'s
  `validate_unique_indexes` (existing) and `validate_nested_path_transforms`
  (added earlier in this same campaign, task 3a) — same file, same calling
  convention (called from both `set_table_schema` and `add_schema_rule`),
  same `err_code(...)`-style error mapping. Add a sibling
  `validate_no_self_referential_cascade` (or similar name consistent with
  this file's conventions) that rejects a schema rule declaring
  `on_delete: Cascade` where the FK's `ref_table` equals the table being
  defined on. Use a coded error name consistent with this codebase's
  snake_case convention (e.g. `self_referential_cascade_not_supported`).
  **Do NOT touch the existing runtime skip for self-referential Cascade in
  `fk_actions.rs`** — leave it exactly as-is (still silently skipping) as a
  defense-in-depth fallback for any pre-existing schema from before this
  DDL check existed, matching task 3a's own precedent for the same
  reasoning (see that task's brief,
  `docs/dev-artifacts/prompts/honesty-fixes/01-ddl-time-nested-path-and-call-in-tx-rejection.md`,
  "Out of scope" section, if you want to see the exact precedent).

### Tests

1. **Site A (RESTRICT)**: `employees.manager_id REFERENCES employees.id ON
   DELETE RESTRICT` — deleting a manager who still has a subordinate row
   referencing them must be REJECTED with `fk_restrict`. Deleting an
   employee with NO subordinates must succeed.
2. **Site B (ON UPDATE)**: for each of Restrict/Cascade/SetNull, a
   self-referential `ON UPDATE` action must fire correctly when a parent's
   `ref_field` value changes (mirror the existing non-self-ref
   `fk_on_update_tests.rs` test shapes for each action, but with a
   self-referential schema). Include the self-loop edge case flagged
   above: verify a row that is simultaneously "part of the updated parent
   set" and NOT itself incorrectly re-matched as its own child (construct
   a small hierarchy — e.g. 3 self-referencing rows — and update TWO of
   the parent keys in one operation, confirming only the genuinely
   dependent child rows are affected, not the co-updated parent rows
   themselves).
3. **Site C (SetNull enabled)**: `employees.manager_id REFERENCES
   employees.id ON DELETE SET NULL` — deleting a manager with direct
   subordinates must SET NULL the subordinates' `manager_id`, not silently
   leave the dangling reference. Confirm this does NOT recurse (a
   subordinate whose OWN subordinates exist should have THEIR
   `manager_id` untouched — SetNull is single-level, matching existing
   non-self-ref SetNull semantics elsewhere in this file).
4. **Site C (Cascade rejected at DDL time)**: attempt to declare
   `employees.manager_id REFERENCES employees.id ON DELETE CASCADE` via
   `set_table_schema`/`add_schema_rule` — must be REJECTED with the new
   coded error, not silently accepted. Confirm the error fires at DDL
   time, before any delete is attempted.
5. Regression: all EXISTING (non-self-referential) FK restrict/cascade/
   setnull/on-update tests continue to pass unchanged — this campaign has
   already fixed several bugs in this exact code (dedup, diamond-cascade
   cycle detection); re-run the full existing FK test suites
   (`fk_actions_tests.rs`, `fk_on_update_tests.rs`, `fk_restrict_tests.rs`)
   and confirm no regression.

## Out of scope

- Do NOT implement genuine multi-level self-referential CASCADE support
  (the cycle-guard redesign) — reject it at DDL time instead, per Site C
  above.
- Do NOT touch `CascadePathGuard` itself, or the diamond-cascade cycle
  detection it implements (task 1c) — this brief works AROUND its current
  table-name-based model, it does not change it.
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, or the
  coercing-set-probes/ScalarResolver-threading/Set-Map-structural-equality
  work (tasks 1a-1e, 2a-2e, 3a, 3b, 3c) — this brief is scoped to findings
  10 and 11 only.

## Verification (MANDATORY before you report done, for BOTH fixes)

- `./scripts/test.sh @engine --full` green, including all new/modified
  tests.
- Confirm whether the DDL-rejection test (Site C, item 4 above) lives under
  `shamir-engine` or `shamir-db` (check where `validate_unique_indexes`'s
  own tests live for precedent) and run the matching scope too if it
  differs, e.g. `./scripts/test.sh -p shamir-db --full`.
- `cargo fmt --all -- --check` clean (or scoped to touched crates, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) Site A and Site B self-referential fixes are
  simple skip-removals with no architectural workaround needed — state
  whether your own reading of the code confirmed or contradicted this
  brief's safety analysis for each; (b) Site C's SetNull-vs-Cascade split
  is implemented correctly (SetNull enabled, Cascade still silently
  skipped at runtime but now rejected at DDL time); (c) both copies of
  `scalar_ref_matches_qv` (fk_actions.rs and fk_on_update.rs) now delegate
  to `scalar_ref_cmp_qv`.
