# Brief 72 — #1065 round 4: write the 7 required tests for real (the write-order fix itself is already committed and verified)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Status — the production fix is DONE, committed, and gate-verified

Commit `eba52e67` on this branch already contains the complete, verified
crash-safe write-order fix for #1065:
- `ddl_op_log` lives in `crates/shamir-index/src/base_index/ddl_op_log.rs`
  (moved from `shamir-engine`, re-exported from `shamir_engine::table::ddl_op_log`
  for existing callers).
- `IndexManager::drop_index` / `drop_unique_index`
  (`crates/shamir-index/src/base_index/index_manager.rs`,
  `index_manager_unique.rs`) write the terminal `Succeeded` status BEFORE
  their own tombstone-clear (`clear_from_dropping`).
- `TableManager::rename_index`
  (`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`) writes
  `Succeeded` BEFORE `clear_from_renaming`, for both regular and unique
  families.
- Recovery paths (`recover_index2_drops`, hash RENAME recovery) write
  `SucceededViaCrashRecovery` before their own tombstone-clear too.
- Client-supplied `request_id` correlation id, idempotent retry (short-
  circuits if a status record already exists for the op_id), `InProgress`
  written before the first mutation, a 1-byte versioned envelope on the
  status record (`ddl_op_log::write_op_status`/`read_op_status` — rejects
  unrecognized versions with a clean `Err`), `log::error!` instead of
  `eprintln!` on status-write failure.
- `cargo fmt --all -- --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  clean, full `./scripts/test.sh` gate for `shamir-index`, `shamir-engine`,
  `shamir-db`, `shamir-server` — **3158/3158 passed**, verified directly
  by the orchestrator (not a self-report).

**Do not touch any of the code cited above.** Your job is ONLY the tests.

## The actual remaining work — 7 required tests, none of which exist in working form

An earlier attempt left a file at
`crates/shamir-db/src/shamir_db/tests/p1065_ddl_status_contract_tests.rs`
that is **currently untracked, NOT wired into the test suite, and would not
compile if wired in**:
- It is not listed in `crates/shamir-db/src/shamir_db/tests/mod.rs` (the
  manifest for that directory — see this repo's CLAUDE.md test-organisation
  convention: "`tests/mod.rs` is a manifest only", every file in that
  directory is `mod`-declared there except this one).
- It has `#[test]` on `async fn` bodies, which is invalid Rust — needs
  `#[tokio::test]`.
- It has `mod helpers;` at the top — no such module exists anywhere under
  `crates/shamir-db/src/shamir_db/tests/`.
- It calls APIs that don't exist in this codebase: `db.create_table("test")`,
  `table.execute(BatchOp::DropIndex(...))`, `table.info_store()`. Verify
  this yourself before writing anything —
  `grep -rn "pub async fn create_table" crates/shamir-db/src`,
  `grep -rn "pub fn info_store" crates/shamir-engine/src` will confirm
  they're not real.
- 3 of its 7 test bodies are empty comments (`// TODO: Implement using...`)
  — no assertions, nothing that could ever fail.

**Delete the fictional content and rewrite the file from scratch** against
the REAL, established API — read
`crates/shamir-db/src/shamir_db/tests/p1_2_ddl_result_contract_tests.rs`
in full first and copy its shape (this is the established, working pattern
for this exact kind of test in this codebase):

```rust
let shamir = ShamirDb::init_memory().await.unwrap();
shamir.create_db("testdb").await;
let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
    .add_table(TableConfig::new("items"));
shamir.add_repo("testdb", repo_config).await.unwrap();
let db = shamir.get_db("testdb").unwrap();
let table = db.get_table("main", "items").await.unwrap();
table.create_index("idx_city", &["city"]).await.unwrap();
```

DDL goes through `shamir_query_builder::batch::Batch` +
`shamir_query_builder::ddl::{drop_index, rename_index}` builders — e.g.
```rust
let mut b = Batch::new();
b.id(1);
b.drop_index("d", ddl::drop_index("idx_city", "items").repo("main"));
let req = b.to_request_via_msgpack();
let resp = shamir.execute("testdb", &req).await.unwrap();
let result = &resp.results["d"];
// result.op_id, result.ddl_status
```
— NOT a fabricated `BatchOp` literal or `table.execute(...)` call.

For `request_id` round-trip / idempotent retry, use the `.request_id(RecordId)`
builder method already landed on `DropIndex`/`RenameIndex`
(`crates/shamir-query-builder/src/ddl/drop_index.rs`, `rename_index.rs`,
from an earlier round of this same task) —
`ddl::drop_index("idx_city", "items").repo("main").request_id(client_supplied_id)`.

For reading the op-status log directly (not just via the wire response),
`crate::table::ddl_op_log::read_op_status` / `write_op_status` need a
`&dyn Store`/`Arc<dyn Store>` — find the established way an existing test
gets a table's info store (grep `read_op_status` usage under
`crates/shamir-engine/src/table/tests/` for the real accessor; it is not
`table.info_store()`).

### The 7 tests to write (all in this one rewritten file)

1. **InProgress written before mutation.** Race a DROP INDEX call against a
   pause hook using `tokio::select!` — **never `tokio::spawn` + `drop`** (read
   `crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs`
   for this codebase's proven pattern for exactly this kind of race). Check
   whether an existing pause-hook seam already parks at the right point
   (after `InProgress` is durable, before the mutating call) —
   `set_drop_index_pause_hook` already exists and is used in
   `crates/shamir-engine/src/table/tests/p1011_reader_drain_tests.rs`; check
   if it parks early enough for this purpose, or whether the existing park
   point is already past `InProgress` (which would make the test trivially
   true and worthless — verify the park point is BEFORE the mutation
   actually starts). Assert via direct `ddl_op_log::read_op_status` that the
   log shows `InProgress` for the `op_id` while parked.
2. **Terminal status durable before tombstone clear, on the INLINE path
   specifically.** This is the test that actually proves this task's fix.
   Race a DROP INDEX call against a pause hook positioned between the new
   status write and `clear_from_dropping` inside `IndexManager::drop_index`
   (`crates/shamir-index/src/base_index/index_manager.rs` — the code is at
   the point cited in "Status" above). If no such hook seam exists in
   `shamir-index`'s test infra yet, add the minimal one, mirroring
   `set_drop_index_pause_hook`'s existing shape (a `BackfillPauseHook`-style
   struct + a setter). Park there, assert `ddl_op_log::read_op_status`
   already shows `Succeeded` for that `op_id`, then let the drop proceed to
   actually clear the tombstone. **Mandatory self-check**: temporarily move
   the status write in `index_manager.rs` back to AFTER `clear_from_dropping`
   (revert the fix locally), confirm this test goes red, then restore the
   fix and confirm it goes green again. Report both observations.
3. **Status-write failure is not silently swallowed.** Check whether any
   `dyn Store` test double in this workspace supports failure injection
   (`grep -rn "impl Store for" crates/` for every implementation and read
   each for a `fail_next`/similar toggle). If one exists, use it to make a
   single `set()` call fail during a DROP INDEX and assert the caller can
   observe something beyond a bare success — re-read what the ORIGINAL
   brief (`docs/dev-artifacts/prompts/release-blockers/69-p1065-ddl-status-crash-safety.md`)
   actually asked for here (it predates the log::error! calls already
   landed) and write the test against that actual requirement — don't
   invent a stronger contract that doesn't exist in the code. If no
   failure-injection seam exists anywhere in the workspace, report exactly
   what you checked and why none qualifies, rather than skip the test
   silently or fake a passing assertion.
4. **Client-supplied correlation id round-trips.** DROP INDEX with a
   supplied `request_id` → the returned `op_id` equals it → polling
   `ddl_op_log::read_op_status` (or the wire `GetDdlOpStatus` request) by
   that same id finds the `Succeeded` status.
5. **Idempotent retry.** Send the SAME DROP INDEX request (same
   `request_id`) twice. Assert the second call does NOT re-execute the drop
   (e.g. assert some observable side effect only happens once, or that the
   second call's response indicates the short-circuit path) and returns the
   SAME `op_id`/status as the first.
6. **Versioned envelope round-trips + rejects unrecognized versions.** Write
   then read a `DdlOpStatus` via `ddl_op_log::write_op_status`/`read_op_status`
   directly, assert correct decode. Separately, write a raw record with a
   corrupted/future version byte directly to the store at
   `ddl_op_log::op_status_key(&op_id)` and assert `read_op_status` returns a
   clean `Err`, not a panic or silent misdecode.
7. **RENAME `DdlOpKind` family classification.** Rename a regular index and
   a unique index (separately), assert the logged `DdlOpKind` for each
   matches its actual family (`RenameHashIndex` for regular,
   `RenameUniqueHashIndex` for unique) — this exercises a pre/post-mutation
   check-ordering change from an earlier round of this task
   (`table.unique_index_exists(&op.rename_index)`, checked on the OLD name
   BEFORE the rename, not the new name after).

**Every test must actually run under `./scripts/test.sh` and FAIL on code
lacking the mechanism it proves.** For tests 2, 4, 5, 6, and 7 specifically,
this is checkable by temporarily reverting the relevant behavior and
confirming red, then restoring — do this for at least test 2 (mandatory,
per above) and report the outcome; for the others, reasoning about why the
assertion is tight enough is acceptable if a literal revert-and-check isn't
practical.

Once all 7 tests exist and compile, add
`mod p1065_ddl_status_contract_tests;` to
`crates/shamir-db/src/shamir_db/tests/mod.rs`.

## Gate before you report done — run every one of these yourself

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- p1065
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
```

Paste the actual final summary line from each `./scripts/test.sh`
invocation (pass/fail counts) — not a paraphrase, the literal output line.
List all 7 tests by name with individual pass/fail status. If anything
fails, fix it before reporting done. Two prior rounds on this exact task
reported success that direct verification disproved (missing test wiring,
invalid syntax, calls to nonexistent APIs, an unrun gate) — the standard
for this round is that every claim you make is something you personally
watched pass, with the command's actual output as evidence.
