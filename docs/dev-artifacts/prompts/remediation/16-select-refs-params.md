# Brief — B14 SELECT refs and params

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

- `crates/shamir-engine/src/query/read/select_projection.rs`, `crates/shamir-engine/src/query/read/aggregate.rs`
- `crates/shamir-engine/src/query/filter/query_ref_cache.rs` and its test file `crates/shamir-engine/src/query/filter/tests/query_ref_cache_tests.rs`
- `crates/shamir-query-types/src/batch/planner.rs` — ONLY the dependency extraction for `Read` queries (the place where dependencies are taken from WHERE only, around :277-281); another task changed `max_nesting_depth_of_ops` in this file already, do not touch that
- projection call sites in `crates/shamir-engine/src/table/read_exec.rs` and `read_planner.rs` ONLY if the new signature requires it; do NOT edit `read_index_scan.rs`, `read_asof_seek.rs`, `read_temporal.rs`, `order.rs` (another agent owns them) — if a call site there needs a change, say so in your final message
- tests: new topic files next to the code (`crates/shamir-engine/src/query/read/tests/` or the existing test dir for these modules) + one-line `pub mod`

## Task

B14 (GitHub issue #47, readable with `gh issue view 47`) — SELECT expressions must receive resolved query references and params.

Observed on master: `select_projection.rs` (~:207, :242-246) evaluates `Expression` / `Function` select items with EMPTY resolved refs and no params (no `with_params`); `aggregate.rs` (~:1109-1110) does the same for function slots; the batch planner (`planner.rs` ~:277-281) takes the dependencies of a `Read` from its WHERE clause only, so `SELECT v + @q1.m AS total` is not ordered after `q1`. A test (`query_ref_cache_tests.rs` ~:383-388) currently PINS the wrong result (`Null`).

Required result:
1. `project_value` and the function slots in `aggregate.rs` build their `FilterContext` from the caller's resolved refs and params plus their own caches. Change the signature (`SelectProjection::new(select, interner, ctx)` or `project_value(record, ctx)` — choose the smaller change after reading the call sites) and update the call sites. The `QueryRefCache` must live per execution (freshness: no stickiness across ForEach iterations / repeated executions).
2. The planner extracts dependencies also from `SelectItem::Function`, `SelectItem::Expression` (via the existing `to_filter_value` conversion) and aggregate arguments, so a query referencing `@q1` / `$query` in SELECT is staged after `q1`.
3. Red tests (deterministic): (a) `q2 = SELECT v + @q1.m AS total` is non-Null AND planned in a later stage than q1; (b) a ForEach over `[1, 2]` with `SELECT $param.row AS tag` returns 1 and 2 (no cache stickiness); (c) an aggregate whose argument uses `$query`/`@q` resolves; (d) replace the Null-pinning test at `query_ref_cache_tests.rs` ~:383 with the correct expectation. Build all queries with `shamir-query-builder`.
4. Keep behaviour identical for selects without refs/params.
