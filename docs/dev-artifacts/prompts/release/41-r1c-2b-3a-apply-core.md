# Brief: R1c step 2b-3a — `ApplyApplicationSchemaMigration` core (tables, indexes, schema rules; no recovery)

Design (settled, read first): `docs/dev-artifacts/design/app-schema-migrations.md` — §3.3, §4.1 (runner sequence; never record an
operation complete before its postcondition is observed; never mark applied before all completions and final checks are
durable), §4.2 (durable progress; retry contract per DDL kind), §4.3 (one migration at a time, fencing generation, the
database maintenance fence and `schema_migration_in_progress`), §4.5 (apply is primary-only), §5.2 (authorization: `Manage`
on the database PLUS the normal authorization of every underlying DDL target; HMAC-required destructive operations are not
bypassed), §5.3 (stable codes), §7.1 (server tests), §8 (layout paragraph). Landed and available: the typed protocol
(`SchemaMigrationApplyRequest` / `SchemaMigrationApplyResult`, the closed `SchemaMigrationOp` subset, canonical checksum),
the journal and fencing of step 2a (`SystemStore::acquire_schema_migration_fence`, `append_schema_migration_event`,
`release_schema_migration_fence`, `read_schema_migration_history`), and steps 2b-1 / 2b-2: the `ApplicationSchema` admin
`BatchOp`, the status and plan handlers in `shamir-db/src/shamir_db/execute/admin_db/` (`schema_migration.rs`,
`schema_migration_plan.rs`), the server follower gate and the error mapping. Read both handlers first and mirror their wiring
and their validation order; the apply arm currently returns the documented temporary error
`schema_migration_operation_not_implemented`.

## Why

Apply is the operation that actually changes a database's application schema in an ordered, journaled, fenced and verifiable
way. It is large, so it lands in two steps: this one (3a) executes the deterministic DDL subset with full durability and
fencing; step 3b adds resume/reconcile of an interrupted migration and the function/validator operations.

## What (this step only)

1. **Apply arm.** Replace the temporary arm for `ApplyApplicationSchemaMigration` with the real handler, in its own file next
   to the status and plan handlers (name the concrete home; count the entries of the folder first).
2. **Admission, in this order, nothing executed before all of it holds:** `Manage` on the database through the normal access
   path; primary-only (the server follower gate already refuses `BatchOp::ApplicationSchema`; keep the engine-side check too);
   serving identity matches the request target; server-recomputed canonical checksum of the submitted body; the expected
   predecessor equals the durable journal head; ordinal contiguous with the applied prefix (the same refusals and stable codes
   as plan); an already-applied identical migration returns the stored applied result as a no-op after re-verifying
   id/ordinal/checksum, a changed body for an applied ordinal is refused without any write; an in-flight owner returns
   `schema_migration_in_progress`; an in-doubt journal returns `schema_migration_in_doubt` (resume is step 3b). Operations of
   the function and validator families are refused with one typed, documented reason in this step (step 3b executes them);
   table, index and schema-rule operations are executed. Any admission refusal writes nothing.
3. **Fence and journal.** Take the per-database fence with the durable compare-and-set generation of step 2a (the request's
   `expected_fence_generation` is checked against it) and an in-process per-database gate; persist and sync `started` with the
   complete checksum-verified body before any effect; then per operation: persist operation intent, execute exactly one bounded
   operation, verify its postcondition, persist operation completion; after the last operation verify end-state invariants,
   persist `applied`, release the fence. If any journal write, sync or read-back fails, stop before the next effect and report
   the migration in doubt (never success from an in-memory cache). The operation id is derived from migration id, ordinal and
   operation index.
4. **Executing an operation.** Reuse the existing DDL handlers/executor paths for the typed operation (create/drop/rename table,
   create/drop/rename index, set table schema, add/remove schema rule) so existing validation, authorization of the DDL target
   and HMAC gates apply unchanged — the migration-level `Manage` never replaces them; an HMAC-required operation that the caller
   cannot satisfy stops the migration BEFORE its intent is written, with the typed refusal. Per design §4.2, each operation
   has an exact postcondition check: create → the object exists with the intended definition (a different existing definition
   is a conflict, never `if_not_exists` success); drop → absent; rename → source absent and destination present with the
   expected identity; index → family, fields/options and terminal state verified; schema rule → the stored rule set equals the
   intended one. A failed postcondition records no completion and returns `schema_migration_postcondition_failed` with the
   journal left resumable (the actual resume is step 3b).
5. **Maintenance fence for writers.** While a migration holds the database fence, other sessions' writes and DDL on that
   database are refused with the stable `schema_migration_in_progress`; reads stay available. Enforce it at ONE choke point of
   the database's batch execution entry, conservatively: a batch from any other session that contains a write, a DDL operation,
   or a stored-procedure call (a call may write through the direct intrinsics) is refused; the migration's own operations pass.
   If you find a write path that cannot be gated at that point, stop and list it with the rationale instead of advertising
   safety. No engine `repo`/`table`/`tx`/`query` layer changes (design §8): if a needed engine primitive is missing, stop and
   list it.
6. **Result.** Build `SchemaMigrationApplyResult` (migration id, ordinal, checksum, status, applied time, fence generation) as
   the admin record with the same wire convention as status and plan. Everything trusted (actor, time, binary version, format
   epoch) is derived by the server, nothing from the client.
7. **Server.** Follower/read-replica refusal is already done by the single gate; extend the error mapping only for codes this
   step introduces to the wire (§5.3 set is complete; add no new code).
8. **Tests** (CI-only; homes named per the layout rule; builder-only queries; see "Test fixture checklist"): a migration with
   several operations applies, the journal shows started → intents/completions → applied → fence released, and the objects
   exist; retry of an applied migration is a no-op; a changed body for an applied ordinal is refused without a write; wrong
   predecessor / gap / checksum mismatch / identity mismatch refused without a write; `Read`-only or non-owner caller refused,
   owner admitted; a DDL target the caller may not touch stops the migration before intent; function and validator operations
   refused with the documented reason; a failed postcondition leaves `started` + intent without completion and reports the
   stable code; journal write failure injected through the 2a seam never returns applied; concurrent apply of the same ordinal —
   exactly one runs, the other gets `schema_migration_in_progress` or the applied no-op; writes, DDL and calls from another
   session during a held fence are refused while reads pass; the same writes are admitted after release; a follower refuses.

## Test fixture checklist (every one of these failed in CI on previous steps — check each before you finish)

* The journal and the fence need a PERSISTENT system store: use `ShamirDb::init(SystemStoreConfig::Fjall(tempdir/"system"))`
  (see `tests/ddl_contracts/schema_migrations/helpers.rs::persistent_db`), never `init_memory`.
* A database identity on the wire is a canonical base58 `RecordId` (use the real identity from the store, or
  `RecordId::from_ts_seq`); a UUID-looking literal does not parse.
* `Manage` is owner/admin-only: to test a caller without it use a NON-owner with read access (`mode` other-read bit); the
  owner always holds `Manage`. Do not run the "allowed" half as the system actor — it passes everything.
* A batch is capped at 50 queries.
* The typed model refuses a non-admitted operation at construction; you cannot send one through the builder.
* Every test must be runnable in isolation and must not depend on timing; use deterministic handshakes.

## Do not

Implement resume/reconcile of an interrupted or in-doubt migration, fence takeover, or the function/validator operations
(step 3b); the client tools (R1c-3); data backfills/DML; change the journal record format of step 2a or the types of step 1
beyond a strictly additive optional field you can justify; add a dependency; touch `shamir-engine`'s `repo` / `table` / `tx` /
`query` layers.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only TRACKED
  files, so it does not see the files you create: count the entries of every folder you add to by hand (`ls`) and name the home
  of every new file in the report. Several host folders are exactly at the limit (`execute/admin_db/` and
  `tests/ddl_contracts/schema_migrations/` are both at 7): the split or move is its own, separately described change.
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no impl
  outside the type's module, no widening of an existing item's visibility; keep new items as narrow as the wiring allows and
  list every widening you need. The orchestrator verifies this locally with the rust-guard binary.
* Future size: `Box::pin` large awaits at the call site (a Windows debug-build stack overflow came from exactly this); the
  apply path awaits many DDL handlers — do not inline them into one large future.
* Imports at the top; one primary item per file.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; do not pipe cargo output through `tail`/`head` (a pipe
  hides the exit code) — redirect to a file outside the repository and read it.
* Leave no scratch files in the tree.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what
  is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-query-types`, `shamir-query-builder`, `shamir-db`,
`shamir-server` (and any other crate you touched) are clean and the entry counts of every touched folder are within the limit;
apply is authorized, validated against the durable prefix, fenced, journaled with intent and completion per operation,
verified by postconditions, applied exactly once, refuses on followers and gates other writers during the fence; the tests
above exist and name what they prove. The final report lists the touched files with their folders, every widening, the
decisions taken and what is not covered.
