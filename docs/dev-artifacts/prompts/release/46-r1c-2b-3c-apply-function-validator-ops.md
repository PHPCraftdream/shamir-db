# Brief: R1c step 2b-3c — function and validator operations in `ApplyApplicationSchemaMigration`

Design (settled, read first): `docs/dev-artifacts/design/app-schema-migrations.md` — §4.2 (restart cases and the per-operation retry
contract; the last bullet: "Function/validator replacement: compare exact code/source hash and relevant registration/binding
metadata; do not infer completion from name existence"), §4.3 (server-wide serialization of any migration that contains a function or
validator operation, because their registrations are server-global), §4.4 (`CheckFunction` preflight before replacing a stored
function; a validator that is stricter than the stored rows may only be activated after the affected rows were scanned; if the scan
cannot run under the fence the migration is refused, never silently activated), §5.3 (stable error codes, no new ones).
Landed and available: the apply core (step 3a) and resume/reconcile (step 3b) in
`crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/` — `apply.rs` (admission; `FUNCTION_FAMILY_DEFERRED` refusal at
admission step 8), `apply_operations.rs` (`run_operations`, `run_operation`, `resume_operation`, `execute_operation`,
`verify_postcondition`, `reconcile_pending` with its documented decision table, intent pre-state evidence), `apply_journal.rs`,
`fence.rs` (in-process gate, open-time in-doubt marker) — plus the journal/fence of step 2a, the status and plan handlers and the typed
operation model `SchemaMigrationOp` (`crates/query/shamir-query-types/src/admin/types/schema_migration/model/operation.rs`). Read the
whole folder and its tests first (`tests/ddl_contracts/schema_migrations/{apply,resume,status_plan}/`). The existing function/validator
admin handlers are `execute/admin_ddl/admin_function*.rs` / `admin_validator.rs`; this step calls them, it does not reimplement them.

## Why

Step 3a/3b execute and resume the table/index/schema-rule subset and refuse the function and validator families with a documented
prefix. Releases change stored procedures and validators together with the schema, so the typed model already admits these
operations but nothing can apply them. This step executes them with the same durability, fencing and exactly-once discipline.

## What (this step only)

1. **Replace the deferred refusal** (`FUNCTION_FAMILY_DEFERRED`, admission step 8, the `_ =>` arms in `apply_operations.rs`) with real
   execution of: create / alter / drop / rename function, check function (as an ordered no-effect operation), create / drop / rename
   validator, bind / unbind validator. Each goes through the EXISTING admin handler path (existing validation, per-target
   authorization, HMAC gate) exactly as the DDL subset does. HMAC-required operations keep being refused before any intent.
2. **Preflight, no effect.** Before the fence is acquired (admission), run `CheckFunction` on the source of every create/alter
   function operation that carries code and refuse the migration with its error when it fails; the plan operation must report the same
   failure without any write. A check failure never writes an intent.
3. **Postconditions and reconcile per kind**, written as additional rows of the decision table in the module documentation, with the
   same record-the-pre-state-in-the-intent technique as create table/index (the intent evidence is optional free text; recording a
   stored-code hash or a "pre_state=absent|present" token is allowed, no format change): create/alter function → stored registration
   exists and its code/source hash and the declared options (language, visibility/security, grants) equal the operation's; drop →
   absent; rename → destination carries the hash recorded at intent and source absent; same shapes for validators; bind/unbind → the
   binding (table, validator) is present/absent exactly. "Name exists" is never enough. A completed operation whose postcondition
   no longer holds is refused as drift (`schema_migration_postcondition_failed`), an unprovable intent stays in doubt and blocks later
   operations, exactly as in step 3b. The in-doubt journal marker may carry the operation index (already allowed by the envelope).
4. **Server-wide serialization.** A migration whose body contains any function or validator operation takes, in addition to the
   per-database gate, a server-wide in-process gate (one atomic holder, lock-free, RAII release on every exit path); a second such
   migration on ANY database is refused with `schema_migration_in_progress`. State in the module documentation how this behaves across
   a restart (the durable control records and the open-time in-doubt markers stay per database; say plainly what is and is not covered).
5. **Strict validators.** Bind of a validator onto a table with existing rows must not silently make writes valid while stored rows
   violate the rule. Look at what the bind handler and the validator runtime already offer. If a bounded read-only validation pass over
   the table under the maintenance fence can be done with existing primitives, run it before the bind effect and refuse (code
   `schema_migration_postcondition_failed`, naming table, validator and the first violating record id) when any row violates. If it
   cannot be done safely with existing primitives, refuse bind with the existing `schema_migration_operation_deferred:` prefix and say
   so in the report; do NOT guess and do NOT widen into the engine.
6. **Dependencies (§4.4).** Check whether the typed migration model can carry the author's list of dependent functions/call sites. If it
   does, plan shows it and an unresolved one blocks apply; if it does not, leave it out and list it as open in the report.
7. **Tests** (CI-only, builder-only queries, see the checklist): per kind execution + postcondition; `CheckFunction` failure refuses
   before any intent and before the fence (journal and catalogue untouched); replace-with-different-code is detected as drift on
   resume; crash after intent / after effect / between operations for at least function create, function rename, validator
   create and bind (crash states built DIRECTLY in the journal with `journal_prefix`, never by counting fault ticks); unprovable
   rename stays in doubt; two migrations with function operations on two databases → exactly one runs, the other gets
   `schema_migration_in_progress`; bind with a violating row refused (or the documented deferral); an already applied migration is
   still a no-op; HMAC-gated operation refused before intent; plan shows the preflight failure.

## Do not

Add a stable error code, change the journal record format (optional free-text evidence only), implement down migrations, a server-side
dependency graph, restore, or force-resolve; touch the clients (R1c-3); add a dependency; touch the engine's `repo` / `table` / `tx` /
`query` layers or the function/validator handlers' behavior for non-migration callers (if a needed primitive is missing, stop and list it).

## Layout — plan it BEFORE coding (both folders are exactly at the 7-entry limit)

* `execute/admin_db/schema_migration/` has 7 entries and `apply_operations.rs` has 979 lines. Do the move first, as its own described,
  git-detectable change with no behaviour change: put `apply.rs`, `apply_journal.rs`, `apply_operations.rs` into an `apply/` subfolder
  (re-exports in `mod.rs` keep every path used elsewhere compiling), then add the function-family code as new files in a new
  `function_family/` subfolder next to `apply/` (entries: `apply/`, `function_family/`, `fence.rs`, `mod.rs`, `plan.rs`,
  `schema_migration.rs`). `apply_operations.rs` may not grow.
* `tests/ddl_contracts/schema_migrations/` also has 7 entries: move `fence_tests.rs` and `journal_tests.rs` into a new `journal/`
  subfolder (rename-only), then add the new tests in `function_family/`; keep every test file under 1000 lines.
* `python -I scripts/ci/check_layout.py` counts only TRACKED files; count folder entries by hand (`ls`) and measure `wc -l` of every
  file you touch; name the home of every new file in the report.

## Test fixture checklist (every item failed in CI on earlier steps)

* Persistent system store (`ShamirDb::init(SystemStoreConfig::Fjall(tempdir/"system"))`, see the resume/apply `helpers.rs`), never
  `init_memory`; keep the `TempDir` alive for the whole test (a dropped temp dir deletes the WAL under the engine).
* Create fixture tables/functions as the database OWNER (`execute_as(OWNER, …)`), not as the system actor; `Manage` is owner/admin-only
  and the owner must also own `store(db, "main")` (see `helpers.rs`).
* A batch is capped at 50 queries; build nested values with `QueryValue`, never `doc!`; distinct alias per op; open every table
  (`check_seed_catalogue`) before a facade read; base58 canonical `RecordId`s.
* Close and drop every handle on a Fjall directory before reopening it; one runtime per test; every await bounded; handshakes, not
  sleeps.
* Concurrent applies: the join order is NOT the winner order — assert on the set of outcomes. An already-applied migration answers
  as the durable no-op BEFORE the in-process gate is consulted, so test a held gate with the NEXT migration.
* The journal envelope validates every appended event: a new field/shape must pass `validate_envelope` or the best-effort append is
  silently lost.
* Anything the engine owns holds only a `Weak` back-reference to the engine; `tokio::time::timeout` built outside a runtime panics
  (`let _enter = rt.enter();`); keep async runners small: `Box::pin` each step (a Windows debug test thread has a small stack).

## Rules that bind the change

* rust-guard: `structure`, `modules`, `layers` rules bind changed files: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local
  `use`, no new `#[allow]`, no impl outside the type's module, no widening of an existing item's visibility (list every widening you
  cannot avoid before doing it). Register no campaign.
* Imports at the top; one primary item per file.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; never pipe cargo output through `tail`/`head` — redirect to a file
  outside the repository and read it. Leave no scratch files in the tree.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what is
  missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits. (For the file moves above use a plain filesystem move and tell the
orchestrator in the report; the orchestrator stages them so git detects the renames.)

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-db` (and any other crate touched) are clean; layout limits hold
(entry counts and file sizes reported); a migration containing function and validator operations is applied exactly once, resumed
after a crash without replaying a proven effect, or stays fail-closed in doubt with an exact reason; a failing `CheckFunction` refuses
before any effect; a second function-family migration is refused while one runs; the tests above exist and name what they prove. The
final report lists the touched files with folders, every widening, the decisions taken (decision-table rows, hash evidence format,
server-wide gate and its restart behaviour, strict-validator outcome) and what is not covered.
