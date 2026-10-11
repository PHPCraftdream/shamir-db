# Brief — 1220 freeze source writes during migration

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.

## FILE SCOPE (only these paths may be edited)

- `crates/shamir-index/src/**/write_barrier_flags.rs` (Grep the real path) — new bit `MIGRATION_FREEZE`
- NEW `crates/shamir-engine/src/table/migration_freeze_guard.rs` (one primary export) + `TableManager::begin_migration_freeze()` (add it to the existing table-manager file that owns `begin_write_barrier`, or a small new file next to it) + re-export
- the commit/CRUD gates: `crates/shamir-engine/src/tx/pre_commit.rs` (ONE check right after the unique write lock is taken, ~:559), `crates/shamir-engine/src/table/table_manager_crud.rs` (the non-tx slow paths ~:186-192, :287-292, :446+, :524+)
- the new `TxError::MigrationFrozen { table }` variant (find `enum TxError`) + its mapping: wire code `table_migrating` in `batch_execute.rs` (~:836, a one-line match arm only), `repo_instance.rs` (~:1163, one arm only — do NOT touch the changefeed init region), `db_tx.rs` (~:214), `From<TxError> for DbError`
- `crates/shamir-db/src/**/admin_migration.rs` (the coordinator wiring), docs: the migration gate message, `core.rs` docs, `KNOWN_LIMITATIONS`, README line ~71
- tests: NEW `crates/shamir-db/tests/migration_source_freeze_tests.rs`, engine and index topic files in the respective `tests/` dirs (+ one-line `pub mod`)
- NO commit/WAL/drainer/materialize/version_guard files, NO changefeed files

## Task

Task #1220 — FREEZE writes to the source table for the whole online migration (instead of building write interception). Decision of the design review: do NOT build interception (4-5 PRs, little value while the destination is in-memory only, the shadow-log LSN is not commit-ordered, the coordinator is in-memory). The source is quiesced instead. PR #69 (post-WAL) and #74 (cutover guard) are merged on master, which is your base.

Implementation plan (follow it; verify line numbers against the code first):
1. `write_barrier_flags.rs`: add `MIGRATION_FREEZE = 1 << 7` to `BARRIER_BITS` / `ALL_BITS` and a `try_set(bit)` via `fetch_or` that reports whether the bit was newly set. (The u8 word is full afterwards — say so in a comment.)
2. NEW `table/migration_freeze_guard.rs` + `TableManager::begin_migration_freeze() -> Result<MigrationFreezeGuard, _>`: take `ddl_admission` ONLY while preparing, `try_set` (error if a freeze is already active), `drain_writers()`, take and release the `unique_write_lock`; the RAII guard clears the bit in `Drop`. Do NOT reuse `begin_write_barrier` (it holds `ddl_admission` for the guard's whole lifetime).
3. Check UNDER the lock: `pre_commit.rs` right after `lock_owned().await` (~:559) -> `Err(TxError::MigrationFrozen { table })`; the same check in the non-tx CRUD slow paths of `table_manager_crud.rs`.
4. `TxError::MigrationFrozen` -> wire code `table_migrating`, mapped in the places listed in the scope.
5. `admin_migration.rs`: freeze BEFORE `replicate_interner_from` (~:149); keep the guard in the coordinator; release it on Rollback, on a failed Start (~:184-190) and on EVERY Commit exit including errors after the final drain (~:286-294). A migration on a follower: refuse Start (replicated apply bypasses the freeze) or document it — pick the refusal if it is a one-line check, else document.
6. Docs: the gate message, `core.rs`, `KNOWN_LIMITATIONS`, README line ~71: "source frozen for writes, error `table_migrating`".

Red tests first: shamir-db `migration_source_freeze_tests.rs`: R1 insert after StartMigration -> `table_migrating`, and dst == src after Commit; R2 an interactive tx begun before Start, committed after -> aborts with `table_migrating`; R3 an in-place update is rejected; R4 writes pass again after Rollback and after a failed Start; R5 no stuck freeze after a Commit error that happens after the final drain. Engine: R6 a parked fast-path committer (use `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`) makes `begin_migration_freeze` wait and the snapshot contains the row; R7 a second freeze errors; R8 a unique-index table's slow path rejects under the lock. Index: `MIGRATION_FREEZE` is in `BARRIER_BITS` and `try_set` works.
Known risks to state in the PR description, not fix: FK cascades into a frozen table abort the parent write; DDL on a frozen source is not blocked.
