# Brief — W10 per-row byte budget in read sinks and server refusal

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

- `crates/shamir-engine/src/table/read_exec.rs`: ONLY the three row sinks `read_streaming`, `read_collecting`, `read_counting` (and small private helpers you add next to them); another agent edits the snapshot gate and vector paths of the same file in parallel - do not touch those regions
- `crates/shamir-engine/src/query/filter/eval_context.rs` (carry an optional row-byte meter exactly like W9 carried `max_accumulator_bytes`), `crates/shamir-engine/src/query/batch/exec_scope.rs`, `query_runner.rs`, `authorized.rs` (only the plumbing of the meter into the `FilterContext` of table reads)
- `crates/shamir-server/src/db_handler/handler.rs` (~:560-660, the response-size step with `grow_unchecked`) and `crates/shamir-server/src/db_handler/cursor_handlers.rs` (~:315-345)
- tests: NEW topic files in `crates/shamir-engine/src/table/tests/`, `crates/shamir-engine/src/query/batch/tests/`, `crates/shamir-server/src/db_handler/tests/` (+ one-line `pub mod`)
- NO commit/WAL/drainer files, NO shamir-query-types wire changes

## Task

W10 (task #1261 B12 / issue #45, `gh issue view 45`; remaining steps 2 and 5). Merged on master (your base): W8 (`ExecScope::charge_bytes`, `result_size.rs` estimator, charge on insert into the results map and for containers, only when `ExecPolicy.enforce_result_size`), W9 (`FilterContext::with_max_accumulator_bytes`), W3/W4 (limits + server policy). Read `result_size.rs`, `exec_scope.rs`, the W9 plumbing of the accumulator cap into `FilterContext`, and the three sinks first.

Problem 1 (step 2): W8 charges a result only AFTER the table read materialised it, so one huge SELECT is fully built before the budget fires. The budget must be charged BEFORE the allocation grows, per row, inside the read sinks.
Required: when enforcement is ON, the sinks charge the estimated size of each row (use the W8 estimator or a cheaper per-row bound that is documented as conservative; never an underestimate that lets an over-cap result through) to the SAME root `ExecScope` byte counter, and stop with `BatchError::LimitExceeded { kind: ResultBytes }` as soon as the cap would be crossed (charged <= cap + one row). Reach the sinks without changing every read signature: carry an optional byte-meter handle in `FilterContext` (like W9). Do not double count: W8 charges the finished result at insertion; define and document precisely how the per-row charges and the insertion charge compose (for example the insertion step must not re-charge rows already charged - add an "already charged" marker on the result path or make the insertion charge the difference; assert exact totals in a test). Enforcement OFF (embedded default): the sinks pay one predictable bool/Option check per row, no estimator call.

Problem 2 (step 5): `handler.rs` (~:635-649) absorbs a response that is larger than its upfront reservation with `ByteBudgetGuard::grow_unchecked`, and only cursor pages are refused (`cursor_handlers.rs` ~:315-345). Required: when the real encoded size exceeds the server-clamped `max_result_size`, the server refuses with the existing result-too-large error code instead of silently growing the in-flight reservation; keep `grow_unchecked` only where the overshoot is bounded by construction and say why in one comment, otherwise remove its use there.

Red tests (deterministic accounting, no wall clock; builder crate for queries): cap 1 KiB, SELECT of ~10 KiB rows -> `LimitExceeded{ResultBytes}` and charged bytes <= cap + one row (assert the exact counter); the same read with enforcement OFF passes unchanged; per-row charge + insertion charge compose to the exact expected total for a plain SELECT, a ForEach body and a nested Batch (no double counting); a grouped / ORDER BY (collecting) read and a COUNT-total (counting) read also stop early; server: an over-cap encoded response is refused with the result-too-large code and the in-flight byte reservation is released.
HOT PATH: `read_streaming` / `read_collecting` / `read_counting` per row. Do NOT run benches (state that `select_pipeline` and `read_path_matrix` are owed).
