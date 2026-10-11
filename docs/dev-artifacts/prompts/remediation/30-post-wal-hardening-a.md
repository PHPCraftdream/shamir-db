# Brief — B02 follow-ups A

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

- `crates/shamir-tx/src/version_guard.rs` and `crates/shamir-tx/src/repo_tx_gate.rs` — ONLY the membership sets `active_post_wal` / `orphaned_post_wal` and their accessors
- `crates/shamir-engine/src/tx/drainer.rs` — ONLY the `counter_applied` (and, if it is the same kind of monotone-key set, `index_applied`) membership set and the code that uses it
- `crates/shamir-engine/src/tx/materialize.rs` / `commit_phases.rs` — ONLY the Phase B / Phase C table-lookup consistency fix of item 3 below
- tests: `crates/shamir-tx/src/tests/version_guard_post_wal_tests.rs`, `crates/shamir-engine/src/tx/tests/post_wal_cancel_tests.rs` and NEW topic files next to them (+ one-line `pub mod`)
- NO `begin_grouped` / WAL group-commit files, NO changefeed files, NO `pre_commit.rs`, NO new spawn/TaskTracker (the repo-owned tail is being designed separately)

## Task

B02 follow-ups, part A (task #1268, items 3-6). PR #69 is merged on master (your base): `VersionGuard::wal_accepted()`, `RepoTxGate::{orphaned_post_wal, orphan_max, active_post_wal}`, the drainer completing orphans. A zero-trust review left these mechanical items; the two design items (repo-owned post-WAL tail, the cancellation window inside `begin_grouped`) are NOT yours.

1. Replace the membership sets `active_post_wal`, `orphaned_post_wal` (VersionGuard / RepoTxGate) and the drainer's `counter_applied` by `scc::HashMap<_, (), THasher>` (`shamir_collections::THasher`), keeping the exact semantics and the O(1) accessors. Reason: their keys are monotone, so in a shared `scc::TreeIndex` (B+ tree) every insert hits the right-most leaf (contention). Read `clippy.toml` (scc `len()` is O(N) and banned) before touching any count; keep the existing atomic mirrors. Do not change anything else about when entries are inserted or removed.
2. Tests that are missing (add them, deterministic, in the style of `post_wal_cancel_tests.rs` with its `TEST_POST_WAL_PRE_PUBLISH_HOOK`; remember a Serializable committer parked after WAL accept holds the repo `commit_lock`, so concurrent parked/committing transactions in a test must use `IsolationLevel::Snapshot`): (a) cancellation in the AsyncIndex commit path; (b) a multi-table orphan (a transaction writing two tables, cancelled post-WAL, both tables' data and counters are present after the drain); (c) a concurrent writer to the same record AFTER the cancel, before the drain (the drained orphan must not clobber the later committed value: assert the final value and the posting state); (d) a counter failure on a multi-table orphan does not block durability. If one of these exposes a real bug, STOP and describe it precisely in your final message instead of papering over it in the test.
3. Phase B uses `per_table_mvcc` while Phase C uses `table_by_token` to find the table (materialize / commit_phases). A concurrent DROP TABLE can therefore make an orphan durable without its history write. Verify by reading the code; if it is real, make the orphan-completion path resolve the table through ONE consistent lookup and fail the orphan cleanly (not durable, retried next pass, as for a failed table lookup in B3 of PR #69) when the table is gone; add a test. If it is not reachable, document why in a comment at the lookup and say so in the final message.
4. The legacy `pre_commit_locked` boundary (only the opt-in `CommitVisibility::AsyncIndex`): between `begin_grouped` Ok and `wal_accepted()` there is only `maybe_crash`, which is Ready. Verify and document that claim in a comment at the call site (one short paragraph).
Do NOT run the `tx_pipeline` bench (no local runs); keep the change behaviour-neutral and say in the final message that the bench run is still owed.
