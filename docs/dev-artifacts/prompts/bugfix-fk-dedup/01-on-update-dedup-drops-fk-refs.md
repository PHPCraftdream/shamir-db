# Bug — ON UPDATE FK planner dedups away all but one FK reference per parent field

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

`crates/shamir-engine/src/query/batch/fk_on_update.rs:179-180`:

```rust
relevant_refs.sort_unstable_by(|a, b| a.parent_ref_field.cmp(&b.parent_ref_field));
relevant_refs.dedup_by(|a, b| a.parent_ref_field == b.parent_ref_field);
```

The dedup key is **only** `parent_ref_field`. But `relevant_refs` — the
deduped vec — is subsequently used not just to build the field-name list for
`collect_parent_values`/`new_values` (fine, that part legitimately wants
unique field names), but ALSO for the `by_table` grouping and per-child
probe construction at lines 204-267 (NOT fine): two different FK
references that happen to share a parent field are collapsed to one, so
only ONE of them gets its cascade/setnull/restrict action actually applied.

The comment `// already deduped above` at line 207 shows the dedup was
intended for the *field list* only, not the ref list itself. Compare the
sibling delete path at `crates/shamir-engine/src/query/batch/fk_actions.rs:168-176`,
which handles this correctly today: it dedups only the *derived*
`parent_ref_fields` vector, and keeps the full `action_refs` vector (every
FK reference) intact for the actual per-child work. `fk_on_update.rs` never
got that same split.

### Concrete failure scenarios

1. **Two child tables, same parent field:** `orders.user_id` and
   `sessions.user_id` both declare `ON UPDATE CASCADE` referencing
   `users.id`. Both refs have `parent_ref_field == "id"`. After
   sort+dedup, only ONE of the two refs survives in `relevant_refs`.
   Updating a user's `id` re-keys (say) `orders` but leaves
   `sessions.user_id` pointing at the OLD value — a silent dangling
   reference, no error raised anywhere.

2. **One child table, two FK fields to the same parent field:**
   `messages.sender_id` and `messages.receiver_id` both reference
   `users.id ON UPDATE CASCADE`. Only one of the two fields gets cascaded;
   the other keeps the stale value.

3. **RESTRICT variant is worse:** if the ref that gets silently dropped by
   the dedup is the RESTRICT one, the parent update is **allowed to
   proceed** even though a child still references the old value — the
   declared referential action is not merely partially applied, it is
   entirely unenforced for that reference.

## The fix

Split the dedup exactly the way `fk_actions.rs:168-176`'s delete path
already does: dedup only a *derived* list of unique field names (used for
`collect_parent_values`/`new_values` — the part that legitimately wants
one entry per distinct parent field, since you only need to fetch the
old/new value for a given field once even if multiple FKs reference it).
Do **not** dedup `relevant_refs` itself — every `OnUpdateRef` that exists
must survive into the `by_table` grouping and per-child probe/action
construction at lines 204-267, so each individual FK reference gets its own
cascade/setnull/restrict check and action, even when several references
share the same parent field.

Read `fk_actions.rs:150-180` (or wherever the delete-path equivalent lives
— locate it precisely; the brief author read it as ~168-176 but confirm
the exact lines in your own pass) as the concrete precedent for the
"two lists, one deduped, one not" pattern before writing the fix — mirror
its shape, don't reinvent it.

## Tests (add to the existing FK on-update test suite — find its current
## location and follow its conventions, e.g. `crates/shamir-engine/src/query/batch/tests/` or
## wherever `fk_on_update` tests currently live)

1. **Two child tables, same parent field, CASCADE:** `orders.user_id` and
   `sessions.user_id` both `ON UPDATE CASCADE` → `users.id`. Update a
   user's `id`. Assert **both** `orders` and `sessions` rows are re-keyed
   to the new id — not just one.
2. **One child table, two FK fields, same parent field, CASCADE:**
   `messages.sender_id` and `messages.receiver_id` both →
   `users.id ON UPDATE CASCADE`. Update a user's `id`. Assert **both**
   fields on the affected message rows are updated to the new id.
3. **RESTRICT variant:** construct the scenario where, under the OLD
   (buggy) dedup, the RESTRICT ref would have been the one silently
   dropped — assert the update is correctly **rejected** (RESTRICT fires)
   rather than silently allowed through.
4. **Regression / non-regression:** a single child table with a single FK
   to a parent field (the common case, unaffected by the bug) must
   continue to cascade/setnull/restrict correctly exactly as before.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all new
  tests (find the exact scope the existing fk_on_update tests run under —
  use `@engine` scope or the equivalent `-p shamir-engine` invocation,
  confirm which by checking how existing FK tests are invoked).
- `cargo fmt --all -- --check` clean (or scoped to `shamir-engine` if a
  full-workspace check is slow — report which you ran).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explain briefly why the fix does not affect the delete-path (`fk_actions.rs`)
  behavior at all — this bug and fix are scoped entirely to the on-update
  path.

## Out of scope

- Do NOT touch the delete-path (`fk_actions.rs`) cascade logic — it is
  already correct; only cited here as the precedent pattern to mirror.
- Do NOT attempt to fix self-referential FK enforcement (a separate,
  already-known, lower-priority gap) or FK Int↔F64 type coercion — those
  are separate tasks tracked elsewhere, not in scope for this fix.
- Do NOT touch anything unrelated to `fk_on_update.rs`'s dedup logic and
  its direct test coverage.
