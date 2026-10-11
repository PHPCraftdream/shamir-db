# Brief — W6 runtime work counter

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- `crates/shamir-engine/src/query/batch/batch_execute.rs` — the two stage loops (~:522-573 and ~:635-686) and the single check next to `commit_tx` (~:789); `crates/shamir-engine/src/query/batch/query_runner.rs` — the ForEach entry (pre-check) only; `crates/shamir-engine/src/query/batch/exec_scope.rs` — the work-counter methods
- tests: NEW topic file in `crates/shamir-engine/src/query/batch/tests/` (+ one-line `pub mod`); existing tests only for mechanical updates
- NO shamir-query-types changes (the types and the static gate are merged: `BatchLimits::{max_total_work}`, `LimitExceeded{Work}`, `LimitKind`), NO server files, NO commit/WAL/drainer files

## Task

W6 — the RUNTIME work counter (work-budget cluster #1207; B13 part 2). Merged on master (your base): W2 (`ExecScope { limits, deadline, work: AtomicU64, bytes: AtomicU64 }` carried through the executor, nothing increments `work` yet), W3 (effective limits, clamped nested bodies), W5 (the static gate in `plan()` counts literal loops only). Read `exec_scope.rs`, `batch_execute.rs`, `query_runner.rs` first.

Why: a ForEach whose `over` is a query reference (dynamic) is invisible to the static gate, so the executed amount of work must be counted at runtime.

Required result:
1. `ExecScope::charge_work(n) -> Result<(), BatchError>`: `fetch_add` on `work` (Relaxed is fine for a budget; document), returning `LimitExceeded { kind: Work, actual, max: limits.max_total_work }` when the new total exceeds the cap; `ExecScope::check_work(extra)` that only CHECKS `work + extra <= max` without charging.
2. Charge +1 per executed entry in the two stage loops of `batch_execute.rs` (non-transactional ~:522-573 and transactional ~:635-686), so nested bodies (which share the scope) accumulate into the same counter.
3. Before iteration 0 of a ForEach (in `query_runner.rs`) check `len(over) x static_body_work <= remaining` where `static_body_work` is the number of entries in the loop body tree (use `BatchTreeVisitor`); check only, no charge (the iterations charge themselves).
4. A final `check_work` right BEFORE `commit_tx` next to ~:789 and NEVER inside or after the commit; no check anywhere in `TxCommit`. In the transactional variant an over-budget error leaves no records applied (the existing rollback path).
5. HOT PATH: one relaxed atomic add per executed entry and nothing else; do not add locals to the per-op future beyond what is needed (the stack-overflow history is in `execution_deadline.rs`'s module doc); do NOT run benches (state in the PR that `for_each_eval`, `tx_pipeline`, `batch_stage_parallelism` are owed).
Red tests (deterministic): a dynamic ForEach `over @src[].id` with 10 rows and an inner dynamic loop of 10 = 122 units against `max_total_work = 50` -> `LimitExceeded { Work }`, in the transactional variant no records remain; the same batch under a generous cap succeeds with the exact unit count observable on the scope; the ForEach pre-check rejects BEFORE the first iteration (assert no side effect of iteration 0); a batch exactly at the cap passes (boundary); nothing is charged by `TxCommit`.
