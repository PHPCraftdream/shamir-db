# Brief — S1 read-after-replication matrix

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

- NEW file `crates/shamir-server/src/replication/tests/read_after_replication_tests.rs`
- the one-line `pub mod read_after_replication_tests;` in `crates/shamir-server/src/replication/tests/mod.rs`
- NO product (non-test) code, NO other test files (another agent edits `follower_loop_tests.rs` concurrently)

## Task

[S1] #1215 — follower read-after-replication correctness matrix (test scaffold).

Goal: a deterministic in-process matrix that replicates writes from a leader `ShamirDb` to a follower `ShamirDb` and then READS on the follower, so that replication correctness is finally observable (today no test reads follower data; `follower_loop_tests.rs` only checks the bookmark and the changefeed).

Fixture rules: read `crates/shamir-server/src/replication/tests/follower_loop_tests.rs` and `in_process.rs` first and reuse their patterns — `ShamirDb::init_memory()` for both nodes, `InProcessReplSource` as the leader side, `run_follower_loop` with `FollowerLoopConfig::new(..)` `max_iterations(1)` and `poll_wait_ms(0)` so exactly one pull happens (no long-poll). Wait for the leader's journal with a bounded `tokio::task::yield_now()` loop (iteration limit, no `sleep`, no wall clock). Build every query with `shamir-query-builder` (never raw JSON). Create the same tables on both nodes with the same schema before writing.

Matrix (one `#[tokio::test]` per row, each with a clear name `matrix_<write>_<read>`): writes {insert one row, insert multi-row, update, delete, writes to two tables in one transaction} x reads on the follower {point get by id, full scan, filter, projection, `COUNT(*)`}; plus rows: event for a table that does NOT exist on the follower (the bookmark must NOT advance past it and nothing may be silently applied), a follower table that has a secondary index (lookup through the index must see the replicated row, or the apply must be refused — assert the safe outcome you can justify from the code), and a follower that pre-interned a field name under a low id before replication (the replicated row must read back under the LEADER's field names).

Ignore rule: run through the code to decide, per row, whether it passes on current master. Rows that you expect to FAIL today (because of the known gaps: replicated map records are not translated between interners, `COUNT(*)` reads a counter that replication does not update, unknown tables are skipped, follower indexes are not maintained) get `#[ignore = "#1211"]` / `#[ignore = "#1212"]` / `#[ignore = "#1213"]` according to the task that will fix them (map records -> #1211, COUNT -> #1212, unknown table / follower index -> #1213). Rows you expect to pass must be written so they pass. In your final message list every row with: expected-to-pass or ignored (and which task), and the exact reason, so the orchestrator can check it against CI. Do not weaken an assertion to make a row pass.
