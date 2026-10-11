# Brief — W0 TxExecute engine deadline

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

- `crates/shamir-engine/src/query/batch/interactive_tx.rs` (the comment block around :63-74 that documents `unbounded` as intentional must be rewritten)
- `crates/shamir-engine/src/query/batch/batch_execute.rs` — ONLY `execute_plan_tx` and the immediate deadline construction there (~:600-615)
- tests: a NEW topic file in `crates/shamir-engine/src/query/batch/tests/` (+ one-line `pub mod` in its `mod.rs`); reuse the slow-resolver pattern of `dos_gate_tests.rs` (~:342)
- NO server files (another task owns `tx_handlers.rs` / `tx_registry.rs`), NO `commit_*`/WAL/drainer files

## Task

W0 / B13 part 1 (GitHub issue #46, readable with `gh issue view 46`) — an interactive-transaction `TxExecute` must run under a real engine deadline.

Observed on master: `execute_plan_tx` (`crates/shamir-engine/src/query/batch/batch_execute.rs`, ~:613) passes `ExecutionDeadline::unbounded()`; `interactive_tx.rs` (~:63-74) documents that as intentional. The server only clamps the *requested* `max_execution_time_secs`, so an interactive tx executes with no time bound at all.

Required result:
1. `execute_plan_tx` builds the deadline from the batch limits exactly like the non-interactive `execute_batch` path does (look at how `from_budget_secs` / `ExecutionDeadline` is built there and mirror it; the default budget for embedded callers is the same 30 s). The cooperative checks that already exist in the stage loops then apply. DO NOT add any deadline or time check to commit, to WAL begin or after it, nor to `TxCommit`: the only allowed check points are the existing ones BEFORE `commit_tx`.
2. Rewrite the `interactive_tx.rs` comment so it describes the new behaviour (the old "unbounded is intentional" rationale must be addressed explicitly: say why a bound is now safe — the interactive tx stays open and is rolled back/dropped by the caller on `ExecutionTimedOut`).
3. Red test (deterministic, no wall-clock sleeps; use the slow-resolver pattern from `dos_gate_tests.rs` ~:342 or tokio paused time, whichever that test file already uses): a batch run through `execute_in_open_tx` whose execution exceeds `max_execution_time_secs` returns `ExecutionTimedOut` (today it succeeds); a fast batch through the same entry still succeeds; the transaction handle is still usable/rollback-able afterwards (no panic, no leaked state).
