# Brief — W9 string_agg caps

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

- `crates/shamir-engine/src/query/read/aggregate.rs` — ONLY the string_agg (and any other string-concatenating aggregate) factory/accumulator code (~:845, ~:900 on the old tree; Grep `string_agg`) and the place where the aggregate learns its byte cap
- `crates/shamir-engine/src/query/filter/` — the `FilterContext` struct and its builder methods (a new optional accumulator byte cap), and the code that constructs the context from the execution scope (`table/read_exec.rs`, `query/batch/query_runner.rs`: ONLY the one line that passes the cap, if needed)
- tests: NEW topic file in `crates/shamir-engine/src/query/read/tests/` (+ one-line `pub mod`); a counting-allocator test in the style of `tests/prealloc_caps.rs` (Grep it)
- NO shamir-query-types changes, NO tx/WAL files, NO server files, NO `batch_execute.rs` (the runtime work counter lives there and is being edited by another task)

## Task

W9 (task #1279; work-budget cluster #1207) — the string-concatenating aggregates are not accounted: `string_agg` can build an unbounded string (separator length x rows + values) inside the engine before any result-size check. B14-3 (aggregate over an expression input, PR #99) and W3 (effective limits) are merged on master, your base. HOT PATH: the aggregate accumulation loop (bench `group_by_keys`) — add nothing per row beyond one integer comparison against a precomputed cap; no allocation, no atomics.

Required result:
1. A separator-length limit: a `string_agg` separator longer than 1024 bytes is rejected when the aggregate is CREATED (factory time) with a typed validation error (a clear message naming the limit); the constant lives next to the aggregate code.
2. An accumulator cap: `FilterContext` gets an optional `max_accumulator_bytes` (a builder method + getter; default = `BatchLimits::CEILING.max_result_size`, so a context built without it behaves like the ceiling, never unbounded). The execution passes the effective `max_result_size` of the batch (the effective limits are on the `ExecScope` / request limits since W2/W3) into the context where the read paths already build it. `string_agg` tracks its accumulated length and, when it would exceed the cap, returns a typed error (`LimitExceeded { kind: ResultBytes, actual, max }` if the aggregate error type can carry a `BatchError`, otherwise a Validation error with the same wire meaning; say which and why) instead of growing; the check is one comparison per appended value.
3. Grouped and ungrouped aggregation both honour the cap (the accumulator is per group).
Red tests: a 64 KiB separator is rejected at creation; 200 rows with a 1 KiB separator against a context cap of 100 KiB -> error before the allocation exceeds the cap (assert with a counting allocator that peak growth stays within cap + small slack, in the style of `tests/prealloc_caps.rs`); the same aggregate under the default context (no cap passed) still works for ordinary sizes (control); grouped aggregation caps per group.
Do NOT run benches (state in the PR that `group_by_keys` is owed).
