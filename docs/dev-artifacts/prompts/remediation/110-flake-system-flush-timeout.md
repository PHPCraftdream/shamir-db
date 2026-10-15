# Brief — FLAKE: Windows system-store flush timeout in flush_all (durability_levels_tests::buffered_batch_executes_successfully)

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Layout limits (CLAUDE.md, ratchet): at most 7 direct entries per folder and at most 1000 lines per source file. Existing violations are grandfathered but must not grow: put new logic into NEW files / subfolders instead of extending an over-limit file or folder, and never push a new file past 1000 lines or a folder past 7 entries.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.

## FILE SCOPE (only these paths may be edited)

- `crates/shamir-engine/src/repo/` shutdown / flush path (`repo/clean_horizon/shutdown_impl.rs`, `flush_buffers`, `finish_clean_shutdown`), `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs` (`flush_all` / `flush_all_report`), `crates/shamir-storage/src/storage_fjall.rs` (flush / persist / write worker) — smallest edits; over-limit files net zero or negative
- the test `crates/shamir-db/src/shamir_db/tests/durability_levels_tests.rs` and NEW tests in an existing `tests/` subfolder with room
- NOT in scope: raising any timeout or budget, unrelated refactors

## Task

FLAKE HUNT (project rule: every flaky failure is a real bug, fixed now, never papered over; the timeout must NOT be raised).

Observed on GitHub CI `cargo test lib (windows-latest)` (PR #182 run, 2026-10-15; the same commit passed on a rerun):
`shamir-db shamir_db::tests::durability_levels_tests::buffered_batch_executes_successfully` FAILED after 28.7 s at `durability_levels_tests.rs:124:30` (`shamir.flush_all().await.unwrap()`):
`Storage("unclean shutdown: [(\"__system__/system\", \"Storage backend error: clean shutdown timed out during flush\")]")`.

The test: `ShamirDb::init(SystemStoreConfig::Fjall(..))`, `create_db`, `add_repo` (fjall), ONE insert, `flush_all()`. Only the `__system__/system` repository is listed as failing, so the user repository flushed in time; the system store's flush (inside the shared 15 s shutdown budget of `flush_all_report`) did not finish. A near-empty fjall flush taking more than ~15 s is a stall, not ordinary variance.

REQUIRED
1. Find the real mechanism statically. Candidates to check with evidence: the fjall write worker (`storage_fjall.rs` `WriteWorker`, std thread + `SyncSender`) blocking the flush request behind a queued write or a full bounded channel; `persist(SyncAll)` on Windows waiting on file handles held by another `Database`/`Keyspace` clone of the same path (the system store and user repo share or open the same `Database`?); a `spawn_blocking` flush starved because the test runs on a current-thread runtime (`#[tokio::test]` default) while a blocking task holds the only thread; the drainer / background tasks of the system repo holding a lock that `flush_buffers` waits on; an order issue where the system store is flushed while user-repo background tasks still write into it (catalogue writes) and the flush waits for them; anything Windows-specific (file locking, fsync on directories).
2. Fix the mechanism in production (or in the test harness if the harness itself causes it, e.g. runtime flavour), never by raising the budget. Explain why the fix removes the stall.
3. Add a deterministic regression test that reproduces the stall shape (hooks/barriers, no sleeps) and passes after the fix; keep the original test as is.
