# Brief — W5 static tree gate in plan()

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

- `crates/shamir-query-types/src/batch/planner.rs` — ONLY the `plan()` function region (the `max_queries` / `virtual_units` checks around ~:104-160); other hunks of this file are being edited elsewhere
- `crates/shamir-query-types/src/batch/tree_visitor.rs` (the multiplier) and, if needed, its re-export in `batch/mod.rs`
- tests: a NEW topic file in `crates/shamir-query-types/src/batch/tests/` (+ one-line `pub mod` in its `mod.rs`)
- NO engine files, NO server files, NO `batch_error.rs` / `batch_limits.rs` changes (the types from W1 are merged: `BatchLimits::{max_total_queries, max_total_work}`, `BatchError::LimitExceeded`, `LimitKind`)

## Task

W5 — a STATIC whole-tree gate in `plan()` (work-budget cluster #1207; design agreed in a read-only review). Today `max_queries` is enforced per nesting level only, so a tree whose every level is under the limit can still describe a huge total amount of work. W1 (merged) added `BatchLimits::max_total_queries` and `max_total_work` and `BatchError::LimitExceeded { kind, actual, max }` with `LimitKind`; read `crates/shamir-query-types/src/batch/batch_limits.rs`, `batch_error.rs`, `tree_visitor.rs` and the existing `plan()` first.

Required result:
1. `BatchTreeVisitor` (`tree_visitor.rs`) carries a `multiplier`: the saturating product of the lengths of the LITERAL `over` lists of the enclosing ForEach loops (a non-literal `over` — a query reference — counts as 1: the runtime counter, a later task, covers it). Keep the visitor iterative (no recursion) and cheap; existing users of the visitor must keep working unchanged.
2. `plan()` (after the existing per-level `max_queries` / `virtual_units` checks) walks the whole tree once with the visitor and enforces: `sum(entries over all bodies) <= limits.max_total_queries` -> `LimitExceeded { kind: TotalQueries, actual, max }`; `sum(multiplier of every entry) <= limits.max_total_work` -> `LimitExceeded { kind: Work, actual, max }`. Use saturating arithmetic; `actual` is the real (saturated) number. The existing per-level errors keep their current variants and precedence (do not change behaviour of anything that already passes or fails today).
3. Red tests (pure DTO, no engine): a literal ForEach 10 x 10 with `max_total_work = 50` fails with kind Work (each level alone passes); a tree whose total entries across nested bodies exceed `max_total_queries` while every level is under `max_queries` fails with kind TotalQueries; the same trees under generous limits plan fine; a non-literal `over` does not multiply; saturation does not panic (huge literal lists). Build trees with `shamir-query-builder` if it is already a dev-dependency of the crate, otherwise with the DTO constructors used by the neighbouring planner tests.
Do NOT touch limits clamping or the engine; another task makes the engine pass the effective (clamped) limits into `plan()`.
