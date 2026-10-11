# Brief — #1219 migration cutover guard

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

- `crates/shamir-engine/src/migration/coordinator.rs` and the other files of `crates/shamir-engine/src/migration/` that hold the migration state machine/state persistence (find them with Grep; keep edits inside that directory)
- `crates/shamir-db/src/shamir_db/execute/admin_migration.rs`
- tests: `crates/shamir-engine/src/migration/tests/` (new topic file + one-line `pub mod`), and a `crates/shamir-db` test in its existing tests layout if the admin path needs one

## Task

#1219 — online migration: never cut over with unapplied backlog, drain the journal completely, and make the cutover retryable.

Observed on master: `drain_until_caught_up` returns `DrainCatchUpResult{residual_lag}` but the production caller `crates/shamir-db/src/shamir_db/execute/admin_migration.rs` (~:178-179) ignores it and moves straight to `CutoverReady`. `final_drain_and_commit` (`crates/shamir-engine/src/migration/coordinator.rs`, ~:316-343) does ONE bounded `read_from`, applies only that page, clears the shadow log and moves to `Committed`: a backlog larger than the page cap is destroyed without being applied. After `Committed` the admin layer runs `seed_log_from_data_store`, `bulk_populate_index2` and removes the active migration; a crash between `Committed` and the end of those steps leaves a migration that is terminally committed but unfinished, and a second `final_drain_and_commit` is no longer accepted (it waits for `CutoverReady`).

Required result:
1. Do not enter `CutoverReady` when `residual_lag != 0`: return a typed error / keep the migration in its draining state so the caller can retry.
2. `final_drain_and_commit` reads the journal PAGINATED until it is empty (apply each page in LSN order using the existing apply path — do not reorder), and only then clears the shadow log and commits. A page cap stays as the page size, not as the backlog size.
3. The cutover steps after `Committed` must be idempotent and re-runnable: persist enough state (or derive it) so that running the admin finalisation again after a crash completes `seed_log_from_data_store`, `bulk_populate_index2` and the cleanup instead of dead-ending. Keep the on-disk format change minimal and backward compatible for in-flight experimental migrations; document the state transition in the module docs.
4. Tests (deterministic, no wall clock): backlog of more than 3 pages is fully applied before `Committed` (compare against a sequential model); `residual_lag != 0` refuses cutover; simulated crash after `Committed` (drop the in-memory coordinator, rebuild from persisted state) followed by a retry finishes the remaining steps exactly once.
5. Online migration stays an experimental feature; do not change its default-enabled state here (a separate task owns that).
