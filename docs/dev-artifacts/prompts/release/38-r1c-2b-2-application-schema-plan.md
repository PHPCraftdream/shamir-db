# Brief: R1c step 2b-2 — `ApplicationSchemaPlan` end to end (no side effects)

Design (settled): `docs/dev-artifacts/design/app-schema-migrations.md` — §3.3 (ordering, gaps, status/plan), §4.1 and §4.4
(what a plan must show), §4.5 (followers: plan-for-apply is primary-only), §5.2 (authorization: plan needs `Manage` on the
database), §5.3 (stable error codes), §7.1 (server tests), §8 (layout paragraph). Landed and available: the typed protocol
in `crates/query/shamir-query-types/src/admin/types/schema_migration/` (`SchemaMigrationPlanRequest`,
`SchemaMigrationPlanResult`, `SchemaMigrationPlannedOperation`, `SchemaMigrationWarning`, `validate_migration_set`,
`canonical_body_checksum`); the journal and identity of step 2a (`SystemStore::schema_migration_identity`,
`read_schema_migration_history`); and step 2b-1 on this branch's base — the `ApplicationSchema` admin `BatchOp`, the status
handler `shamir-db/src/shamir_db/execute/admin_db/schema_migration.rs`, the server follower refusal and the error mapping.
Read the status step first and mirror its wiring; do not redo it.

## Why

A client must see exactly what the next migrations would do — checksums verified, order proven against the durable applied
prefix, effects and their blast radius listed — before anything is applied, and without any side effect. Plan is also the
server-side gate the client uses after a lost response, so its refusals must be the stable §5.3 codes.

## What (this step only)

1. **Plan arm of the handler.** Replace the temporary "not implemented" arm for `ApplicationSchemaPlan` (keep apply's
   temporary arm; it is step 2b-3). Put the plan logic in its own file next to the status handler (one primary item per
   file; name the concrete home — count the entries of `execute/admin_db/` first).
2. **Authorization.** `Manage` on the database through the normal `authorize_access` path, before any journal byte is
   read; a caller with only `Read` is refused. No bypass.
3. **Validation, server-side, nothing trusted from the client.** Resolve the serving identity and compare it with the
   request target (`schema_migration_identity_mismatch`). Read the durable history. Recompute every submitted body's
   canonical checksum from the typed body and refuse a mismatch (`schema_migration_checksum_mismatch`). Prove the order
   against the applied prefix: the expected predecessor must equal the journal head (`schema_migration_predecessor_mismatch`),
   the submitted ordinals must continue the prefix contiguously with no gap, duplicate or reorder
   (`schema_migration_gap`), and a submitted migration that is already applied must carry the same id/ordinal/checksum
   (a changed body for an applied ordinal is refused, without any write). An in-flight owner or an in-doubt migration is
   reported with `schema_migration_in_progress` / `schema_migration_in_doubt` and no plan. Run the admitted-subset check
   (`validate_migration_set` and whatever else the types crate exposes); an unsupported operation is refused with a typed
   reason that names the operation.
4. **Result.** Build the typed `SchemaMigrationPlanResult` for the next migration(s): per operation its stable id,
   kind, target and a work bound where one is known; warnings from the existing enum, set by rule (maintenance fence is
   always required for apply; function/validator operations carry the server-wide blast-radius and dependency-review
   warnings and list the affected registrations; data operations that are not admitted are refused, not warned). The
   result is the admin record in the same wire shape as the status handler.
5. **Followers.** Plan is primary-only: on a follower or read replica it fails closed with
   `schema_migration_follower_read_only` (server-side; reuse the single place the status step added).
6. **No effects.** The plan arm writes no journal event, takes no fence, executes no operation. A test must prove it by
   comparing the journal and the catalogue before and after.
7. **Builder.** `shamir-query-builder` gets the matching typed constructor for the plan admin op next to the status one
   (builder-only; no raw JSON).
8. **Tests** (CI-only; homes named per the layout rule): plan allowed with `Manage` and refused with `Read` only;
   no journal or catalogue change after a plan; checksum mismatch; wrong predecessor; gap, duplicate and reorder; a changed
   body for an applied ordinal refused without a write; identity mismatch; in-flight and in-doubt states; unsupported
   operation refused; function/validator warnings and the maintenance-fence warning present; follower refusal; and a wire
   round trip through the normal batch entry point built with `shamir-query-builder`. Reuse the fixtures of the status
   tests (`tests/ddl_contracts/schema_migrations/`).

## Do not

Implement apply, the fence, recovery or operation execution; touch `shamir-engine`'s `repo` / `table` / `tx` / `query`
layers; change the journal record format of step 2a or the types of step 1 beyond a strictly additive optional field you
can justify; add a dependency; introduce a new wire error code (the §5.3 set is complete).

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only
  TRACKED files, so it does not see the files you create: count the entries of every folder you add to by hand
  (`ls`) before you finish, and name the home of every new file in the report. Where the only right home is a full folder,
  say so instead of overfilling it.
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no
  impl outside the type's module; keep new items as narrow as the wiring allows.
* Future size: do not add awaited helpers into the large init or dispatch futures inline — `Box::pin` them at the call
  site (a Windows debug-build stack overflow came from exactly this on 2026-11-17).
* Imports at the top; builder-only queries in tests; multi-line indented JSON literals where JSON is unavoidable.
* Machine memory: run at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`. Do not pipe cargo output through `tail`/`head`
  (a pipe hides the exit code); redirect to a file and read the file.
* Leave no scratch files in the tree (logs, notes); write them outside the repository.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final
  report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-query-types`, `shamir-query-builder`, `shamir-db`,
`shamir-server` (and any other crate you touched) are clean and the entry counts of every touched folder are within the
limit; the plan arm is authorized with `Manage`, validates and orders against the durable prefix with the stable codes,
returns the typed plan, writes nothing and refuses on followers; the tests above exist and name what they prove. The final
report lists the touched files with their folders, the decisions taken and what is not covered.
