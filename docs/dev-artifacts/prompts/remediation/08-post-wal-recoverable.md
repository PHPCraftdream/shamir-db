# Brief — B02 post-WAL cancellation recoverable

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

- `crates/shamir-tx/src/version_guard.rs`
- `crates/shamir-tx/src/repo_tx_gate.rs`
- `crates/shamir-engine/src/tx/commit.rs`, `crates/shamir-engine/src/tx/materialize.rs`, `crates/shamir-engine/src/tx/drainer.rs`
- tests: a NEW file `crates/shamir-engine/src/tx/tests/post_wal_cancel_tests.rs` plus its one-line `pub mod` in that directory's `mod.rs`; unit tests for the guard in `crates/shamir-tx/src/tests/` (new topic file plus the one-line `pub mod`) if needed

## Task

B02 (GitHub issue #35, readable with `gh issue view 35`) — a commit cancelled AFTER the WAL accepted it must stay recoverable. STEP 1 ONLY (a repo-owned background tail is a later, separate task).

Observed on master: `crates/shamir-tx/src/version_guard.rs` :100-117 — `Drop` marks the version Aborted in both trackers. `crates/shamir-engine/src/tx/commit.rs` :970-1008 and `materialize.rs` :76-246 `.await` between "WAL accepted the batch" and `VersionGuard::commit()`. If the commit future is cancelled in that window the guard aborts a version that is already durable in the WAL; `drainer.rs` :387 and :396-404 then skips the cancelled version, and overlay GC plus WAL truncation drop it: acknowledged-to-WAL data is lost.

Required result:
1. `VersionGuard::wal_accepted(&mut self)` moves the guard to a post-WAL state in which `Drop` does NOT mark the version Aborted. The version is registered in `RepoTxGate::orphaned_post_wal` (an `scc::TreeIndex` of orphan versions plus an `AtomicU64` max).
2. The drainer's scan bound becomes `max(visible, orphan_max)`; the drainer itself completes orphan versions (marks Materialized, advances `last_committed`, marks durable). `CounterDelta` is replayed only for orphans that do not carry the "5b applied" flag (find how step 5b records that; keep the replay idempotent).
3. Call `wal_accepted()` right after the WAL write in `commit.rs` (:984 and :987) and in `pre_commit_locked` (the legacy path).
4. Red tests (`post_wal_cancel_tests.rs`), deterministic via a test hook `TEST_POST_WAL_PRE_PUBLISH_HOOK` modelled on the existing `fire_post_validate_pre_publish_test_hook` (search for it): after the WAL offer and Phase 5a: tx1 commits; tx2 is spawned, the test waits for `hook.reached`, then `abort()`s the task; tx3 commits; `drain_all`; force a WAL segment seal; drain/truncate again; expect tx2's record readable, `durable_watermark == last_committed`; then reopen the disk repo and expect A, B, C and B's index posting. Also a PRE-WAL cancel control: B is absent, the watermarks advance, locks are released.
5. Do not change the public commit API. Keep the hot path lock-free (no new mutex).
