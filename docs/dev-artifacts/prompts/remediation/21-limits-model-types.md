# Brief — W1 limits model types

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

- `crates/shamir-query-types/src/batch/batch_limits.rs` (find the real file with Grep for `struct BatchLimits`), and the new types in NEW files next to it: `exec_policy.rs` (ExecPolicy), `limit_kind.rs` (LimitKind) — one primary export per file, re-exported from `batch/mod.rs`
- `crates/shamir-query-types/src/batch/batch_error.rs` (the new `LimitExceeded` variant + `Display`)
- `crates/shamir-server/src/db_handler/handler.rs` — ONLY one new branch in the `error_code` mapping (~:715) for `BatchError::LimitExceeded`
- fix full `BatchLimits { .. }` struct literals that no longer compile: `crates/shamir-query-types/src/batch/tests/planner_tests.rs` (~:152, :224) and `crates/shamir-query-builder/.../batch_tests.rs` (~:538) — Grep for other exhaustive literals and use `..BatchLimits::default()`
- tests: new topic file in `crates/shamir-query-types/src/batch/tests/` (+ one-line `pub mod`)
- NO engine/executor changes, NO server config changes

## Task

W1 — the types of the server-owned limit model (no behaviour change). Part of the "work budget" cluster (#1207, B11-B13); design agreed in a read-only review.

Required result:
1. `BatchLimits` gets four new fields, all `#[serde(default = ...)]` so an OLD six-field payload still deserializes: `max_total_queries`, `max_total_work`, `max_page_size`, `max_offset`. Defaults: pick generous finite defaults that do not change the behaviour of any existing test (e.g. total queries = 10_000, total work = 1_000_000, page size 100_000, offset 1_000_000 — match the pagination ceilings that already exist as constants in `crates/shamir-engine/src/query/batch/batch_validate.rs`; read them and reuse the same numbers). Keep `Default` consistent.
2. `BatchLimits::clamped_to(&self, cap: &BatchLimits) -> BatchLimits`: per-field minimum (all numeric fields).
3. `impl BatchLimits { pub const CEILING: BatchLimits }` — the hard engine ceiling nobody can raise; `max_queries: 1000` (keeps the planner's recursive DFS shallow), the other fields set to sane large-but-finite values. Document in one line why 1000.
4. `ExecPolicy { caps: BatchLimits, enforce_result_size: bool }` with `ExecPolicy::EMBEDDED = { caps: CEILING, enforce_result_size: false }` (a `const`/associated const or function — whichever compiles for a type containing the limits struct). Doc: the policy is server-owned and passed separately from the request DTO, a client cannot influence it.
5. `LimitKind` (enum: Nodes/TotalQueries, Work, Iterations, Depth, PageSize, Offset, ResultBytes — choose names matching the wire codes below) and `BatchError::LimitExceeded { kind: LimitKind, actual: u64, max: u64 }` with a `Display` message. Wire error codes in the server `error_code` mapping: PageSize/Offset -> `pagination_too_large`, Work/TotalQueries -> `work_budget_exceeded`, ResultBytes -> `result_too_large`, Depth -> `nesting_too_deep`, Iterations -> `too_many_iterations` (reuse the existing code strings that the corresponding old variants already map to; read the mapping first).
6. Tests: an old six-field msgpack/serde payload deserializes with defaults for the new fields; `clamped_to` takes the minimum per field and is idempotent; `CEILING.clamped_to(&x)` never exceeds CEILING; `LimitExceeded` maps to the expected wire codes (a small unit test next to the existing `error_code` tests if there is one).
