# Brief for F-27b (#827, P1) — restore the OLD validator artifact when an ALTER's activation fails

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-24 (#817, already landed, commit `cc2630e7`) fixed `compile_table_schema`
(`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs`) for the
**fresh-registration** case: if a table's FIRST-EVER schema fails to fully
activate (a later step — `get_table` / `add_validator_binding` — fails
after `ValidatorRegistry::register` already succeeded), the fresh
registration is undone, restoring the registry to its pre-call state,
symmetric with the caller's catalogue `rec_prev` rollback.

F-24 explicitly did **not** touch the **ALTER** case
(`ValidatorRegistry::replace_artifact` branch — `schema_validator_id`
already existed, e.g. an existing table's schema being modified) — by
design, since undoing a `replace_artifact` requires restoring the OLD
compiled validator, not just removing an entry, and F-24 was scoped to the
narrower fresh-registration leak. Read `compile_table_schema`'s current
code (post-F-24) in full before starting — the shape today is:

```rust
let freshly_registered = self.validators.id_for_name(&name).is_none();
if freshly_registered {
    self.validators.register(schema_validator_id, &name, validator).map_err(...)?;
} else {
    let replaced = self.validators.replace_artifact(&schema_validator_id, validator);
    if !replaced { return Err(...) /* stale name collision, F-24 hardening */ }
}

let activation_result: DbResult<()> = async {
    let table = self.get_table(db_name, repo_name, table_name).await?;
    // ... add_validator_binding, add_binding ...
    Ok(())
}.await;

if activation_result.is_err() && freshly_registered {
    self.validators.remove(&schema_validator_id);
}
activation_result
```

**The remaining gap**: in the ALTER branch (`freshly_registered == false`),
`replace_artifact` immediately overwrites the LIVE compiled validator with
the NEW (attempted) rules — this validator starts being enforced by
concurrent writers on this table THE MOMENT `replace_artifact` runs, well
before `activation_result` is even known. If a LATER step then fails, the
caller (`admin_schema.rs`) rolls the CATALOGUE back to `rec_prev` (the OLD
schema's data, unchanged `schema_validator_id`) — but the registry now
holds the NEW rules' compiled validator under that SAME id, forever (until
the next successful schema mutation for this table, if any). **The
catalogue says the OLD schema is active; the live enforcement gate
actually runs the NEW (never-fully-activated) schema's rules.** This is a
genuine persisted/live state divergence, not just a memory leak.

Confirm this by reading `ValidatorRegistry::replace_artifact`
(`crates/shamir-engine/src/validator/registry.rs` ~line 93-97) — it's an
in-place RCU swap (`by_id.update_sync`), so the OLD compiled `Arc<dyn
RecordValidator>` is gone (dropped) the instant the swap succeeds; nothing
elsewhere in the codebase retains a reference to it.

## The fix — capture the OLD artifact before swapping, restore it on later failure

The simplest correct design: capture the current live artifact via
`ValidatorRegistry::get_by_id` **before** calling `replace_artifact`, and
if a later step in the same call fails, put it straight back via a SECOND
`replace_artifact` call. No need to re-derive/recompile anything from
`rec_prev`'s stored rules — the exact `Arc` that was live a moment ago is
restored byte-for-byte:

```rust
let freshly_registered = self.validators.id_for_name(&name).is_none();

// Capture the artifact that's about to be replaced, so it can be put back
// if a later step fails — see the restore at the bottom of this function.
let prior_artifact: Option<Arc<dyn RecordValidator>> = if freshly_registered {
    None
} else {
    self.validators.get_by_id(&schema_validator_id)
};

if freshly_registered {
    self.validators.register(schema_validator_id, &name, validator).map_err(...)?;
} else {
    let replaced = self.validators.replace_artifact(&schema_validator_id, validator);
    if !replaced { return Err(...) /* F-24's existing stale-collision guard, unchanged */ }
}

let activation_result: DbResult<()> = async { .. }.await;

if activation_result.is_err() {
    if freshly_registered {
        self.validators.remove(&schema_validator_id);
    } else if let Some(old) = prior_artifact {
        // Put the OLD compiled validator back — symmetric with the
        // catalogue's rec_prev rollback, which still points at this same
        // schema_validator_id expecting the OLD rules to be what's live.
        self.validators.replace_artifact(&schema_validator_id, old);
    }
}
activation_result
```

Investigate one edge case before finalizing: could `prior_artifact` ever
legitimately be `None` when `freshly_registered` is `false`? That would
mean `id_for_name` found a name→id mapping but `get_by_id` found no
compiled artifact for that id — an inconsistent registry state that
shouldn't normally occur (registration always inserts both maps together).
Decide whether this needs an explicit `debug_assert!`/log, or whether
silently doing nothing (no artifact to restore) is fine — use judgment,
but don't invent elaborate handling for a state the registry's own
invariants should already prevent.

## Tests

Extend `crates/shamir-db/src/shamir_db/shamir_db/tests/schema_rollback_tests.rs`
(F-24's test file, same module — this is the direct continuation of that
work) with:

1. **Core regression — the divergence this task closes**: register an
   "old" validator artifact under `schema_validator_id` (simulating a
   pre-existing, active schema — mirror F-24's own
   `alter_replace_artifact_is_not_undone_on_failure` test setup), call
   `compile_table_schema` with NEW rules for a table that will fail at a
   later step (reuse the same "table doesn't exist" trick F-24's tests
   already use for `get_table` failure), then assert the registry's live
   artifact for that id is **the OLD one again**, not the new (failed)
   one — this requires a way to distinguish the two artifacts at the test
   level (e.g. give the OLD and NEW `SchemaValidator`s different rule sets
   and assert via `get_by_id(&id)` + a validation call, or via `Arc::ptr_eq`
   against the originally-registered `Arc` if `SchemaValidator`/
   `RecordValidator` doesn't expose its rules directly — check what's
   feasible and use the most direct assertion available).
2. **Fresh-registration case is unaffected** — re-run (or confirm still
   passes unchanged) F-24's existing `fresh_register_is_undone_when_activation_fails`
   test; this task must not alter that code path's behavior.
3. **Successful ALTER still works** — a full happy-path ALTER (old
   artifact replaced, activation succeeds) must still end with the NEW
   artifact live, not the old one — a regression guard that the restore
   logic only fires on `activation_result.is_err()`.

## Docs

Update `docs/guide-docs/KNOWN_LIMITATIONS.md`'s F-24 entry (search for
"F-24, #817") — either extend it to note this ALTER-path gap is now ALSO
closed by F-27b/#827, or add a short adjacent bullet — whichever reads
more naturally given how the existing F-24 bullet is structured.

## Constraints

- Do NOT touch the fresh-registration path's behavior (F-24) — only the
  ALTER (`replace_artifact`) branch changes.
- Do NOT touch the 3 `admin_schema.rs` call sites — this fix is fully
  self-contained inside `compile_table_schema`.
- Do NOT touch F-27a's interner-persist-rollback fix (#820, already
  landed) — unrelated code path, no interaction.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -- --check` and
  `cargo clippy -p shamir-db --all-targets -- -D warnings` must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- schema_rollback
./scripts/test.sh -p shamir-db --full
```
