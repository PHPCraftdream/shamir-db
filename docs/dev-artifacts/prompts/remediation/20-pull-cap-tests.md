# Brief — W0b Pull cap tests

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

- `crates/shamir-server/src/db_handler/repl_handler.rs` and `crates/shamir-server/src/replication/in_process.rs` — ONLY to replace the duplicated Pull event cap with ONE shared constant (define it where both can reach it, e.g. in `repl_handler.rs` as `pub(crate) const MAX_PULL_EVENTS`)
- tests: NEW topic files: `crates/shamir-server/src/replication/tests/pull_cap_tests.rs` (+ one-line `pub mod` in that `mod.rs`) and, for the wire handler, a new file in `crates/shamir-server/src/db_handler/tests/` (+ one-line `pub mod`)
- NO `changelog_store.rs` / `shamir-tx` changes (already merged and covered by their own tests)

## Task

W0b / B11 remainder — characterization tests for the replication Pull bounds that were merged without nonempty-data tests.

Observed on master (PR #73 merged): `repl_handler.rs` (~:165) and `replication/in_process.rs` (~:96) clamp the Pull event count to a hard cap and the long-poll wait to 30 s; `ChangelogStore::range_from` stops at a fixed 16 MiB byte budget (returns a resumable prefix; an oversized FIRST entry is rejected). The cap constant is duplicated in the two files, and the only tests use an EMPTY journal, so the event cap and the byte cap are not proven on real data.

Required result:
1. One shared constant for the event cap used by both call sites (no behaviour change).
2. Tests on real data (deterministic, in-process `ShamirDb::init_memory`, fixtures like the existing replication tests; bounded `yield_now` loops with an iteration limit instead of sleeps): (a) a journal with 2000 committed single-row events -> a Pull asking for `u32::MAX` events returns at most the cap, in order, starting at the requested version, and the next Pull continues without gap or duplicate; (b) a journal whose events together exceed the 16 MiB byte budget (use large payloads, keep the test fast) -> the Pull returns a non-empty prefix below the budget and the next Pull resumes at the right version; (c) the same two properties through the wire handler path (`DbRequest::Pull` handled by the replication handler, with a session that is allowed to pull) for both the in-process source and the handler; (d) `wait_ms` above the cap is clamped (assert on the value actually used if observable; if not observable without a sleep, skip this one and say so).
These are characterization tests that should PASS on current master; if one fails, that is a finding — report it precisely in your final message instead of weakening the test.
