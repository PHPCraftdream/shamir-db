# Brief — B14-3 aggregate over an expression

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

- `crates/shamir-query-types/src/read/agg.rs` (the aggregate representation: add an optional per-row input expression) and the other query-types files that must follow from it (builder-facing constructors, serde); `crates/shamir-query-builder` (the aggregate builders)
- `crates/shamir-engine/src/query/read/aggregate.rs` (accumulation: evaluate the input per row against the `FilterContext`; the Vec-returning legacy wrappers' fate, see below)
- `crates/shamir-query-types/src/batch/planner.rs` — ONLY the dependency-discovery code (where B14 step 1 extracts dependencies from SELECT functions/expressions/aggregate arguments); do NOT touch `plan()`'s limit checks (another PR edits that hunk)
- tests: NEW topic files in the respective `tests/` dirs (+ one-line `pub mod`)
- NO tx/WAL files, NO `table/read_exec.rs` index2 blocks (another task edits them), NO limit/budget code

## Task

B14-3 (task #1280, GitHub issue #47) — an aggregate must be able to take an EXPRESSION as its per-row input, e.g. `SELECT SUM(v * @q1.m)` or `AVG(price * $param.rate)`. B14 steps 1-2 are merged on master (your base): the projection and the aggregate FACTORY arguments (percentile fraction, string_agg separator) accept execution constants, but `AggregateFn.args` (`crates/shamir-query-types/src/read/agg.rs` ~:21-25) is factory configuration, not per-row input, and accumulation (`query/read/aggregate.rs` ~:1055-1120) receives only a field name or `All`.

Required result:
1. Representation: an aggregate takes an input that is either a field (today) or an expression (`FilterValue`-style expression, the same type the projection uses for `SelectItem::Expression`). Keep the wire format of existing aggregates unchanged (an additive, optional field; an old payload still deserializes — add a test).
2. Execution: the per-row input expression is evaluated against the row AND the execution's `FilterContext` (resolved query refs and params), for grouped and ungrouped aggregation on every table read path that already forwards the context (B14 step 2). Rows where the expression yields Null follow the existing NULL-skipping rule of the aggregate (document which). A type error is a typed error, not a silent skip.
3. `shamir-query-builder`: constructors for aggregates over an expression, e.g. a `sum_of(expr)` next to the existing `sum(field)`; no hand-written JSON anywhere.
4. Planner: dependency discovery covers the new input expression (an `@q1.m` inside it orders the query after `q1`), exactly like B14 step 1 does for projections.
5. The legacy Vec-returning wrappers `apply_group_by` / `apply_aggregate_all` (used only by benches and tests) currently `unwrap_or_default()` on error: make them fail loudly instead (they may return the typed error, or panic with a clear message if changing the signature breaks benches — pick the smaller change, update the callers in benches/tests mechanically, and say which).
Red tests: `SUM(v * @q1.m)` returns the resolved value and is ordered after q1; `AVG` with a `$param` multiplier in a ForEach over [1, 2] gives different results per iteration (no cache stickiness); grouped aggregation over an expression; Null and type-error behaviour; old payload compatibility; the planner dependency test.
