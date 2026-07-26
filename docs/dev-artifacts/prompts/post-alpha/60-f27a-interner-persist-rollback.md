# Brief for F-27a (#820, P1) — `interner_mgr.persist()` failure needs a catalogue rollback

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`crates/shamir-db/src/shamir_db/execute/admin_schema.rs` has 3 schema-DDL
handlers (`handle_set_table_schema`, `handle_add_schema_rule`,
`handle_remove_schema_rule`) that all follow the SAME sequence:

```rust
// 1. Persist the catalogue record with the NEW schema fields.
self.shamir.system_store().save_table_meta(&rec).await
    .map_err(|e| err_code("internal_error", e.to_string()))?;

// 2. Durably persist the repo interner (newly-interned field-path ids).
interner_mgr.persist().await
    .map_err(|e| err_code("internal_error", e.to_string()))?;   // <-- NO ROLLBACK

// 3. Activate: compile + register + bind the validator (live, in-process).
if let Err(activation_err) = self.shamir.compile_table_schema(...).await {
    // Rolls the catalogue back to rec_prev on failure. Correct, already tested.
    if let Err(rollback_err) = self.shamir.system_store().save_table_meta(&rec_prev).await { .. }
    return Err(...);
}
```

(Exact line numbers: `handle_set_table_schema` ~483-519,
`handle_add_schema_rule` ~645-675, `handle_remove_schema_rule` — same
shape, search for `.persist()` in this file to find all 3.)

Step 3's failure path (`compile_table_schema`) already rolls the
catalogue back to `rec_prev` — this was F-4 (#794)'s fix, already landed
and tested. **Step 2's failure path has no rollback at all** — if
`interner_mgr.persist()` fails, the function just returns the error via
`?`, but `save_table_meta(&rec)` (step 1) has ALREADY durably written the
NEW schema to the catalogue. That new schema's field-path ids were
interned in-memory (via `parse_schema`/`serialise_rules` earlier in the
same handler) but are NOT yet durable — if the process crashes before any
LATER successful interner persist, a restart's `boot_compile_schemas`
(`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs` ~line
536+) would try to de-intern the catalogue's schema field using a
`repo_interner` that never learned those new path ids, likely failing to
resolve them (check `boot_compile_schemas`'s actual behavior on an
unresolvable path — does it skip the table's schema entirely, silently
losing enforcement, or hard-fail boot? verify and state precisely in your
summary).

This is the SAME class of gap F-4 already closed for `compile_table_schema`'s
failure — just left unfixed one step earlier, for `interner_mgr.persist()`'s
failure specifically. A **separate, broader** task (F-27b, #827) covers a
harder, related problem (an ALTER's live validator artifact not being
restored on a later failure) — do NOT attempt that here; this task is
scoped ONLY to giving `interner_mgr.persist()`'s failure the same
catalogue-rollback treatment `compile_table_schema`'s failure already has.

## The fix

At all 3 call sites, wrap `interner_mgr.persist()`'s failure with the SAME
compensating-rollback pattern already used for `compile_table_schema`'s
failure — restore `save_table_meta(&rec_prev)`, log a warning if THAT also
fails (matching the existing warning message style/wording at each site,
just naming "interner persist" instead of "activation" as the trigger),
and return the original `interner_mgr.persist()` error to the caller
(mirroring how the activation-rollback path returns `activation_err`, not
`rollback_err`).

Since `rec_prev` is already captured earlier in each handler (for the
LATER `compile_table_schema` rollback), no new snapshot is needed — just
reuse the existing `rec_prev` variable that's already in scope at the
`interner_mgr.persist()` call site in all 3 functions.

## Tests

Find or add a fault-injection seam for `interner_mgr.persist()` failing
(check whether `RepoInternerManager`/whatever `interner_mgr`'s type is has
an existing test double, error-injection hook, or whether the simplest
approach is a storage-backend that fails writes — check how F-4's own
`compile_table_schema`-failure rollback tests forced THEIR failure, for a
reusable pattern in this same test module/crate) and add, for at least
`handle_set_table_schema` (the other 2 can follow the same pattern if the
fault-injection seam is generic enough — use judgment on how many of the 3
sites need their own dedicated test vs. one parametrized/shared test
helper):

1. **Core regression**: `interner_mgr.persist()` fails after
   `save_table_meta(&rec)` succeeded → the catalogue is rolled back to
   `rec_prev` (assert the table's catalogue record afterward matches the
   PRE-mutation state, e.g. `schema_version` unchanged, `SCHEMA_FIELD`
   unchanged).
2. Confirm the ORIGINAL `interner_mgr.persist()` error (not a rollback
   error) is what's returned to the caller — matching the existing
   activation-rollback behavior's error-precedence convention.

## Constraints

- Do NOT touch `compile_table_schema` or its own rollback path (F-24,
  already landed) — this task only adds a rollback for the STEP BEFORE it.
- Do NOT attempt the ALTER-path live-validator-artifact-restore problem —
  that is F-27b (#827), a separate, larger task.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -- --check` and
  `cargo clippy -p shamir-db --all-targets -- -D warnings` must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- schema
./scripts/test.sh -p shamir-db --full
```
