# Brief — W3 engine enforces the limit set

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

- the engine executor files that W2 (already in your worktree: PR #87, branch `refactor/exec-scope`) threads `ExecScope` through: `crates/shamir-engine/src/query/batch/batch_execute.rs`, `query_runner.rs`, `batch_validate.rs`, `interactive_tx.rs`, `exec_scope.rs`, `authorized.rs`
- tests: NEW topic files in `crates/shamir-engine/src/query/batch/tests/` (+ one-line `pub mod` in its `mod.rs`); existing tests only for mechanical signature updates
- NO `shamir-query-types` changes (types are merged), NO planner/tree_visitor files (another task edits `planner.rs`), NO server files, NO commit/WAL/drainer files

## Task

W3 — the engine enforces the whole limit set (work-budget cluster #1207). Your worktree is built on top of W2 (PR #87, `ExecScope` carried through the executor chain and `Authorized` carrying an `ExecPolicy`); read `crates/shamir-engine/src/query/batch/exec_scope.rs` and `authorized.rs` and the types in `crates/shamir-query-types/src/batch/{batch_limits,exec_policy,batch_error}.rs` first.

Observed on master (verify): nested bodies choose their own limits — `batch_execute.rs` ~:167 and `query_runner.rs` ~:719, :885, :919 take `body.limits` as is, so a nested Batch with `max_queries = usize::MAX` escapes the root limit; a ForEach body's `max_iterations` is never clamped by the root; `validate_pagination(queries)` (`batch_validate.rs` ~:52) uses fixed constants and returns an untyped error.

Required result:
1. The EFFECTIVE limits are computed once at the entry (`execute_batch`, `execute_in_open_tx`, i.e. W2's `root_scope()`): request limits AND policy caps AND `BatchLimits::CEILING` (per-field minimum via `clamped_to`; make sure `CEILING` is applied even when the policy is EMBEDDED).
2. A nested Batch / ForEach body gets `body.limits.clamped_to(&effective_root_limits)` — a client can only lower, never raise. A loop's `max_iterations` is `min(body, scope)`.
3. `validate_pagination(queries, &limits)` takes the effective limits and returns the typed `BatchError::LimitExceeded { kind: PageSize | Offset, actual, max }` (reuse the existing wire codes through the server's `error_code` mapping that W1 added; do not change the codes). The old constants live on as the `BatchLimits` defaults (W1) — remove the duplicates.
4. Keep every check where it is today relative to `commit_tx`: nothing new inside or after the commit.
Red tests (write them so they fail on the unclamped code): a nested Batch with `limits.max_queries = usize::MAX` and 60 queries under a root `max_queries = 50` -> `TooManyQueries`; a ForEach whose body sets `max_iterations = u64::MAX` under a root of 2 and 3 iterations -> `TooManyIterations`; an `Authorized` with `ExecPolicy { caps.max_page_size = 10 }` rejects a query with `limit 11` as `LimitExceeded { PageSize }` and accepts `limit 10`; an offset above `max_offset` -> `LimitExceeded { Offset }`; defaults still accept today's maximum pagination values (control). The error shape inside nested bodies may change: update the existing assertions that pinned the old untyped pagination error and list them in your final message.
Do NOT run benches. Do not touch the stack-sensitive per-op frame beyond what W2 already did.
