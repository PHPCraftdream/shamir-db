# Brief — W4b server turns on runtime result-size enforcement

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

- `crates/shamir-server/src/config.rs` (the `security.query_limits` config struct, defaults, validation)
- `crates/shamir-server/src/db_handler/config.rs` (`QueryLimitsCap`, its `policy()` and `From<&QueryLimitsConfig>`)
- other server files ONLY where a `QueryLimitsCap { .. }` struct literal or a `QueryLimitsConfig { .. }` literal must gain the new field (LIST them)
- shipped profiles `deploy/*.ktav` (and any config example/fixture that enumerates query_limits keys)
- docs: ONLY the statements that say server byte enforcement is OFF in `docs/guide-docs/guide/07-operations.md` and `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md` (if those files are not on your base yet because another PR is open, skip the docs and say so)
- tests: NEW topic files in `crates/shamir-server/src/tests/` and `crates/shamir-server/src/db_handler/tests/` (+ one-line `pub mod`)
- NO engine files, NO `read_exec.rs`, NO `handler.rs` / `cursor_handlers.rs` (another agent edits the server response-size step and the engine read sinks concurrently)

## Task

W4b (task #1301; work-budget cluster). Defect found while documenting W8: `QueryLimitsCap::policy()` in `crates/shamir-server/src/db_handler/config.rs` sets `enforce_result_size: false` with the comment "Preserve existing semantics: result size is a reserve estimate". W8 (runtime byte budget, merged: `ExecScope::charge_bytes`, `result_size.rs`) and W10 (per-row charging, in flight) only act when `ExecPolicy.enforce_result_size` is true, so the server NEVER enforces the byte budget at runtime. The embedded API default stays OFF (`ExecPolicy::EMBEDDED`).

Required:
1. Add an operator-visible flag `enforce_result_size: bool` to the server's `security.query_limits` config (serde default TRUE: the point of the budget is protection; document the semantics in the field doc: bytes MATERIALIZED by an execution, estimated conservatively, checked before commit, intermediates and ForEach/nested containers included, no give-back). Wire it `QueryLimitsConfig -> QueryLimitsCap -> policy()`; `QueryLimitsCap::UNLIMITED` keeps enforcement OFF (tests rely on it); add validation if any combination is inconsistent (for example enforcement ON with a `max_result_size_bytes` below what `validate_query_limit!` already allows - read the existing validation and keep it coherent with the reserve-estimate check at ~config.rs:781-790).
2. Update every shipped profile (`deploy/*.ktav`) and example so it parses and passes validation (the W4 lesson: a shipped profile that fails validation is a bug - there are tests that load them; read how `server.large.example.ktav` is tested and extend the same test to the new key).
3. Update the two doc statements that currently say server byte enforcement is OFF (if present on your base).
Tests (red first, deterministic): config parse + default TRUE + explicit false round-trip; `policy().enforce_result_size` follows the flag; a server-handler test that drives a real batch through the handler with a small `max_result_size_bytes` where the MATERIALIZED result (including an intermediate with `return_result = false`) crosses the cap -> error code `result_too_large` mid-execution, a batch below the cap passes, and the same batch with the flag false passes unchanged (follow how `crates/shamir-server/src/db_handler/tests/server_limit_policy_tests.rs` builds its fixtures).
HOT PATH: none (config wiring only). Do NOT run benches.
