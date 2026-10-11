# Brief — B14 step 2 SELECT refs/params on remaining paths

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

- `crates/shamir-engine/src/table/read_exec.rs`, `crates/shamir-engine/src/table/read_temporal.rs`, `crates/shamir-engine/src/query/read/exec.rs`, `crates/shamir-engine/src/query/read/select_projection.rs`, `crates/shamir-engine/src/query/read/aggregate.rs` (and any other file under `crates/shamir-engine/src/table/read_*.rs` that calls `project_value` / builds an aggregate with an empty context — Grep to find them)
- the aggregate-function parameter representation in `shamir-query-types` ONLY if needed to let `AggregateFn` parameters be non-literal (Grep for `AggregateFn`); keep wire/serde compatibility not required (no release yet) but do not break existing tests silently
- tests: NEW topic files next to `crates/shamir-engine/src/query/filter/tests/query_ref_cache_tests.rs` or in `crates/shamir-engine/src/table/tests/` (+ one-line `pub mod` in the `mod.rs`)
- NO planner/batch executor files (`crates/shamir-engine/src/query/batch/*` belongs to another task), NO tx/WAL/drainer/commit files, NO server files

## Task

B14 step 2 (GitHub issue #47, `gh issue view 47`) — finish forwarding the resolved query references and params into every SELECT projection and aggregate path.

Already merged on master (PR #78, commit message readable with `git log -1 01717c55`): `SelectProjection::with_context(&FilterContext)` binds the caller's `resolved_refs` and `params` (cache cells reset when the binding changes), the context-aware projection sites in `read_exec.rs` and ungrouped aggregation get the context, and the batch planner extracts dependencies from SELECT functions / expressions / aggregate arguments.

Remaining (observed, verify each before editing):
1. The index-scan, as-of, temporal (`read_temporal.rs`) and `query/read/exec.rs` read paths still project with an empty context: Grep every `project_value(` call site and every `SelectProjection::new(` and make each one bind the same `FilterContext` the surrounding read already has (or that the caller passes in). If a path has no context available, thread it through the minimal chain of signatures — prefer an explicit parameter over a global/thread-local.
2. `AggregateFn` parameters are literal-only: `SELECT SUM(v * @q1.m)` / `$param` arguments must be evaluated against the context like projection expressions are (grouped aggregation included, not only ungrouped).
3. A path where binding is genuinely impossible must fail loudly (a typed error), never silently evaluate to Null.

Red tests first (write them so they FAIL on current master for the right reason, then fix): for EACH previously uncovered path — index-scan read, as-of read, temporal read, grouped aggregate, aggregate with `$param` argument — a query `SELECT v + @q1.m AS total` (or the aggregate analogue) executed in a batch after `q1` returns the resolved non-Null value, and a ForEach over [1, 2] with `SELECT $param.row AS tag` yields 1 and 2 (no cache stickiness across iterations). Use the builder crate (`shamir-query-builder`) to construct queries; do not hand-write JSON. Read the existing tests added by PR #78 (Grep `resolved_refs` in `crates/shamir-engine/src/**/tests`) and mirror their fixtures.

Report in your final message the exact list of call sites changed and any path you left alone with the reason.
