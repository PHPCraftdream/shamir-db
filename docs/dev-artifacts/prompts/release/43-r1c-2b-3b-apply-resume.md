# Brief: R1c step 2b-3b — resume and reconcile of an interrupted `ApplyApplicationSchemaMigration` (DDL subset, no functions/validators)

Design (settled, read first): `docs/dev-artifacts/design/app-schema-migrations.md` — §4.1 (runner sequence), §4.2 (durable progress,
the four restart cases and the per-DDL retry contract), §4.3 (fencing generation, "recovery must first fence the old owner, then
acquire a new generation and reconcile its pending operation"), §5.3 (a lost response, resume uses the SAME apply operation with the
identical stored body, the expected predecessor and a fresh fence generation; there is no separate reconcile operation), §7.1
(server tests). Landed and available: the apply core of step 3a in
`crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/` (`apply.rs` admission, `apply_operations.rs`
`run_operations` / `verify_postcondition`, `apply_journal.rs` `append_step` / `record_in_doubt`, `fence.rs`), the journal and fence
of step 2a in `crates/engine/shamir-db/src/shamir_db/system_store/catalogue/schema_migrations/` (`journal.rs`:
`acquire_schema_migration_fence(…, recovery: bool, …)` already supports taking over an owned fence for an unapplied migration,
`append_schema_migration_event`, `release_schema_migration_fence`, `read_schema_migration_history`), the status and plan handlers.
Read the apply handler and its tests first: admission step (6) in `apply.rs` currently refuses an unresolved journal with
`schema_migration_in_progress` / `schema_migration_in_doubt`; this step replaces that refusal with resume where it is provably safe.
Operations of the function and validator families stay refused with the documented deferred reason (step 3c); this step does not
execute them and does not reconcile them.

## Why

A crash, a restart or a lost response can leave a migration with durable `started`, intents and completions but no `applied`. Today
the only outcome is a permanent refusal. This step makes the interrupted migration finishable exactly once, or keeps it fail-closed
in doubt when the effect cannot be proven.

## What (this step only)

1. **Resume admission.** Submitting the identical stored body (same id, ordinal, canonical checksum) with the expected predecessor and
   the current durable fence generation, for a migration that has an unresolved journal, enters recovery instead of refusing:
   * a different body, ordinal or id than the unresolved migration is refused (stable codes of §5.3, no write);
   * a live in-process runner of the same database (the in-process gate is held) is never taken over: `schema_migration_in_progress`;
   * otherwise take over with the durable compare-and-set (`recovery = true`): the old owner is fenced by the new generation, the
     body and checksum are re-verified against the journal, and every later append carries the new generation; a stale runner that
     wakes up after the takeover cannot append and cannot execute another operation (prove it with the 2a failure seam).
2. **Reconcile per operation (§4.2 cases 1–4), DDL subset only** (create/drop/rename table, create/drop/rename index, set table
   schema, add/remove schema rule): from the journal and the live catalogue classify each operation index as completed / intent
   without completion / not started and act:
   * completed → re-verify the postcondition; if it no longer holds, refuse with `schema_migration_postcondition_failed` and the
     drift described (never conceal drift, never re-run);
   * intent without completion → decide from the exact postcondition and the recorded pre-state: desired state proven → append
     completion WITHOUT replaying the effect; proven not to have taken effect → execute once under the new fence, verify,
     complete; neither proven (for example rename with both names present or neither present, drop where the absence cannot be
     tied to the recorded target) → keep/set `in_doubt`, block every later operation, return `schema_migration_in_doubt` naming the
     operation index and the reason. Write the decision table per operation kind as the module documentation of the new file;
   * not started → run through the normal path (intent, one bounded operation, postcondition, completion).
   If the intent record does not carry enough to prove the pre-state for some kind, a strictly additive optional field of the
   intent payload is allowed ONLY if you justify it in the report (format epoch and old readers unchanged); otherwise leave that
   shape in doubt rather than guess.
3. **Finish.** After the last completion re-verify the end-state invariants, append `applied` under the new generation, release the
   fence, return the same `SchemaMigrationApplyResult` shape (the fence generation reported is the new one). A migration whose
   operations are all complete but whose `applied` never became durable is finished by resume (verify, append `applied`).
4. **Maintenance fence across restart.** While the durable control record says the fence is owned (a runner crashed or the server
   restarted), other sessions' writes, DDL and stored-procedure calls on that database must still be refused (reads pass), with
   `schema_migration_in_progress` for a live runner and `schema_migration_in_doubt` when no runner exists. Find the cheapest correct
   way to rebuild the in-process fence state from the durable control records when the database catalogue is loaded (a small
   one-time scan at system-store load, never a durable read per batch). If this needs a change outside `shamir-db`'s system-store
   load / the 3a fence module, stop and list it with the rationale instead of widening.
5. **Status.** `ApplicationSchemaStatus` must report the pending operation index, its kind and the in-doubt reason for an unresolved
   migration (read the status handler; extend its typed result only additively and keep the wire shape; if it already reports this,
   add the missing test only).
6. **Tests** (CI-only; homes named per the layout rule; builder-only queries; see "Test fixture checklist"). Simulate crashes with the
   2a failure seam and by dropping the database handle and reopening the SAME persistent system store directory (close every handle
   first; one runtime per test): crash after the intent but before the effect → resume runs the effect once; crash after the effect
   before the completion → resume proves it and completes WITHOUT a second effect (observable: table/index identity unchanged, no
   second build, journal has exactly one completion); crash between two operations; crash after the last completion before `applied`
   → resume appends `applied` only; unprovable operation (rename with both names present) stays `in_doubt` and blocks later
   operations, status shows it, an identical resubmission does not change the state; completed operation drifted → refused with
   the drift, no re-run; different body / ordinal / id refused with no write; stale runner fenced after takeover; two concurrent
   resumes → exactly one runs, the other gets `schema_migration_in_progress` or the applied no-op; writers blocked after restart
   until the resume finishes, admitted after; follower refuses; resumed result shape equals a normal apply's; an applied migration
   retried is still a no-op.

## Do not

Execute or reconcile function/validator operations (step 3c), add a separate reconcile/resolve operation, force-resolve an in-doubt
migration, implement restore or down migrations, change the journal record format beyond the single additive optional field above,
touch the clients (R1c-3), add a dependency, touch `shamir-engine`'s `repo` / `table` / `tx` / `query` layers (if a needed engine
primitive is missing, stop and list it). Add no new stable error code (§5.3 is complete).

## Test fixture checklist (every one of these failed in CI on earlier steps — check each before you finish)

* The journal and the fence need a PERSISTENT system store: `ShamirDb::init(SystemStoreConfig::Fjall(tempdir/"system"))` (see
  `tests/ddl_contracts/schema_migrations/helpers.rs::persistent_db` and the apply `helpers.rs`), never `init_memory`.
* A database identity on the wire is a canonical base58 `RecordId`; a UUID-looking literal does not parse. `Manage` is owner/admin-only:
  the owner of the database must ALSO own the repository resource `store(db, "main")` created in the fixture (grant it as the 3a
  `helpers.rs` does) or table creation is denied.
* A batch is capped at 50 queries; never build nested test values with `doc!` — use `QueryValue`; give every op of a batch builder a
  distinct alias.
* Open every table of a fixture (including the system tables, via `check_seed_catalogue`) before a facade reads it.
* Close and drop every handle on a Fjall directory before the same directory is opened again; keep one runtime alive across a test
  that spawns engine tasks. Tests are deterministic: handshakes, not sleeps; every await in a test is bounded.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only TRACKED files:
  count the entries of every folder you add to by hand (`ls`), measure `wc -l` of every file you touch, name the home of every new
  file in the report. `schema_migration/` and the apply test folders are at or next to the limit: plan the layout BEFORE coding;
  a move/split of existing files is its own, separately described change (keep git-detectable renames, no behaviour change in the
  move).
* rust-guard: `structure` and `conservation` run only for registered campaigns; register NONE. `structure`/`modules`/`layers` rules
  still bind changed files: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no impl
  outside the type's module, no widening of an existing item's visibility (list every widening you cannot avoid before doing it).
* Future size: `Box::pin` large awaits at the call site; do not inline the DDL handlers into one large future.
* Imports at the top; one primary item per file.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; do not pipe cargo output through `tail`/`head` — redirect to a
  file outside the repository and read it.
* Leave no scratch files in the tree.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-db` (and any other crate you touched) are clean and the entry
counts of every touched folder and file sizes are within the limits; an interrupted migration of the DDL subset is finished exactly
once or stays fail-closed in doubt with an exact reason; no effect is replayed after its postcondition was proven; the old owner is
fenced; writers stay blocked until resolution, also after a restart; the tests above exist and name what they prove. The final report
lists the touched files with their folders, every widening, the decisions taken (decision table, the additive field if any, how the
fence is rebuilt after restart) and what is not covered.
