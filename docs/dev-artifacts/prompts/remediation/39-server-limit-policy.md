# Brief — W4 server wiring of the limit policy

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

- `crates/shamir-server/src/config.rs` (`QueryLimitsConfig`, `Config::validate` ~:360-411), `crates/shamir-server/src/db_handler/config.rs` (`QueryLimitsCap::policy()` ~:24-38), `crates/shamir-server/src/server_launcher.rs` (wire the config into the handler, ~:473-477)
- `crates/shamir-server/src/db_handler/handler.rs` (the three-field clamp block ~:490-506 and the `execute` path) and `crates/shamir-server/src/db_handler/tx_handlers.rs` (the same clamp block, the `TxExecute` path)
- the `shamir-db` entry points that the handlers call (Grep `execute_as` / `tx_execute_as` in `crates/shamir-db/src/shamir_db/execute/`): add `execute_with_policy` / `tx_execute_with_policy` variants that take an `ExecPolicy` and mint the `Authorized` token `.with_policy(policy)`; the old names keep working with `ExecPolicy::EMBEDDED`
- docs for the new config fields (the server config reference — Grep for `max_execution_time_secs` in `docs/`)
- tests: NEW topic files in `crates/shamir-server/src/db_handler/tests/` and `crates/shamir-server/src/tests/` (+ one-line `pub mod`); harness as `byte_budget_exhaustion_tests.rs`
- NO engine files (`crates/shamir-engine/**`), NO shamir-query-types changes

## Task

W4 — wire the server-owned limit policy (work-budget cluster #1207). Merged on master (your base): W1 types (`BatchLimits`, `ExecPolicy`, `CEILING`), W2 (`Authorized::with_policy`), W3 (the engine clamps request AND policy caps AND CEILING and nested bodies), W5, W7. Read `crates/shamir-query-types/src/batch/{batch_limits,exec_policy}.rs`, `crates/shamir-engine/src/query/batch/authorized.rs`, then the server files.

Today the server clamps only `max_result_size`, `max_execution_time_secs` and `max_queries` (`handler.rs` ~:490-506, a copy in `tx_handlers.rs` ~:89-101); `max_nesting_depth`, `max_dependency_depth`, `max_iterations` and the new total/pagination limits are not server-capped at all.

Required result:
1. `QueryLimitsConfig` (config.rs): new fields for EVERY `BatchLimits` field the operator may cap — `max_nesting_depth`, `max_dependency_depth`, `max_iterations`, `max_total_queries`, `max_total_work`, `max_page_size`, `max_offset` next to the three existing ones — with defaults equal to the current effective behaviour (no behaviour change for an unmodified config), `serde(default)`, and `Config::validate` rejects a value above the engine ceiling (`BatchLimits::CEILING`) with a clear message (and zero where zero makes no sense).
2. `QueryLimitsCap::policy() -> ExecPolicy`: `ExecPolicy { caps: BatchLimits built from the config, enforce_result_size: <existing semantics of the result-size cap> }`; wire it in `server_launcher.rs`.
3. The handlers call `execute_with_policy` / `tx_execute_with_policy` (new variants in `shamir-db`, see the scope) and the three-field clamp blocks in `handler.rs` and `tx_handlers.rs` are DELETED — the engine clamps now. The server keeps only whatever estimate it needs for the RI-15 reserve (read how the reserve uses the clamped limits first and keep exactly that behaviour). Keep the `TxExecute` lifetime logic of W7 untouched.
4. Pagination ceilings (B11 rest): `max_page_size` / `max_offset` come from the config through the same policy.
5. Document the new fields in the server config reference.
Red tests (harness style of `byte_budget_exhaustion_tests.rs`): a client sending `max_nesting_depth = 1000` above a server cap of N with a deeper nest gets the `nesting_too_deep` wire code; same for `max_iterations` (`too_many_iterations`), a page size above `max_page_size` (`pagination_too_large`), `max_total_work` (`work_budget_exceeded`); an unmodified default config keeps accepting everything it accepted before (control); `Config::validate` rejects a field above the ceiling; the TxExecute path applies the same policy.
