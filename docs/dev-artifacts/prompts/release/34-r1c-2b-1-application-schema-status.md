# Brief: R1c step 2b-1 — `ApplicationSchemaStatus` end to end (read-only)

Design (settled): `docs/dev-artifacts/design/app-schema-migrations.md` — §5.2 (server operations, authorization), §5.3
(stable error codes), §4.5 (followers), §8 (step 2 lands as 2a + 2b; 2b is split here into status / plan / apply),
the layout-constraint paragraph of §8, §7.1 (server tests). Landed and available: step 1 — the typed protocol in
`crates/query/shamir-query-types/src/admin/types/schema_migration/` (`SchemaMigrationAdminOp` with the three operations,
`SchemaMigrationStatusRequest` / `SchemaMigrationStatusResult`, `SchemaMigrationErrorCode`, canonical body and checksum);
step 2a — database identity and the durable journal in
`crates/engine/shamir-db/src/shamir_db/system_store/catalogue/schema_migrations/` (`SystemStore::schema_migration_identity`,
`read_schema_migration_history` returning `MigrationHistory`, fencing and event append used by later steps).

## Why

Clients and the server's own tooling need to read where a database's application schema stands (applied prefix, drift,
in-flight owner, in-doubt migration) before anything may be applied. Read-only status is the smallest end-to-end slice
that proves the wiring (wire → authorization → handler → durable journal → typed result → server error mapping) that
`plan` and `apply` then reuse.

## What (this step only)

1. **Wire.** Make `SchemaMigrationAdminOp` a typed admin batch operation (a new `BatchOp` variant in
   `crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs` following the way the most recent comparable admin
   operation was added end to end — find it, mirror its wiring, and list every file you touched in the final report).
   Only the status arm is implemented now; the plan and apply arms return one clearly named, documented temporary error
   that the next steps replace (no new wire error code for it). The operation names stay `ApplicationSchema*` /
   `ApplyApplicationSchemaMigration`; never `StartMigration` / `MigrationStatus`.
2. **Handler in `shamir-db`.** Resolve the target database, read its identity and history through the step 2a API and
   build `SchemaMigrationStatusResult`: last applied id / ordinal / checksum, checksum validation against what the request
   carries, in-flight owner and in-doubt operation, and the identity-mismatch outcome (`schema_migration_identity_mismatch`)
   when the journal's identity does not match the serving database. Everything trusted (actor, time, versions) is derived
   by the server; nothing is taken from the client.
3. **Authorization.** `Read` on the database (design §5.2) through the normal `required_access` / `authorize_access` path;
   no bypass.
4. **Server.** On a follower or read replica status is refused as stale (`schema_migration_follower_read_only`), design
   §4.5; map handler errors to the stable codes of §5.3 so they stay distinguishable from the existing online
   storage-move errors; one place for the mapping.
5. **Tests** (CI-only), placed per the layout constraint (name the concrete home of every new file; where the only right
   home is a full folder, the split or move is its own commit): status of an empty journal; after applied events staged
   through the 2a append API; with an owned fence; in doubt; checksum mismatch; identity mismatch; `Read` allowed and a
   caller without access refused; follower refusal; a wire round trip through the normal batch entry point built with
   `shamir-query-builder`, never raw JSON.

## Do not

Implement plan or apply logic, the client tools (R1c-3), format-epoch injection (that belongs to apply), or touch
`shamir-engine`'s `repo` / `table` / `tx` / `query` layers (design §8: none; if you find a needed engine primitive, stop
and list it with the rationale in the final report). Add no dependency. Do not change the journal record format of step 2a.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file (`python -I scripts/ci/check_layout.py`); several host
  folders are exactly at the limit (see the design's list) — count before adding.
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules (tests in `tests/` with a manifest
  `mod.rs`), no local `use`, no new `#[allow]`, no impl outside the type's module; keep new items as narrow as the wiring
  allows. Imports at the top. One primary type per file.
* Future size: do not add awaited helpers into the large init or dispatch futures inline — `Box::pin` them at the call
  site (a Windows debug-build stack overflow came from exactly this on 2026-11-17).
* Machine memory: run at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`.
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

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for every crate you touched (`shamir-query-types`, `shamir-db`,
`shamir-server`, plus dependents of a changed public signature) and the layout gate are clean; the status operation is
reachable through the normal batch entry point with the Read check; plan and apply are explicit temporary errors; the tests
listed above exist and name what they prove. The final report lists the touched files, the decisions taken, and open
questions.
