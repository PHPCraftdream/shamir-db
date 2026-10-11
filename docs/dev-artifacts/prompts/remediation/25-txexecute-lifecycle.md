# Brief — W7 TxExecute lifecycle

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

- `crates/shamir-server/src/tx_registry.rs` (InteractiveTx lifetime accessor), `crates/shamir-server/src/db_handler/tx_handlers.rs` (TxExecute lifecycle only: the lifetime check at entry, the call-time computation, and the error-to-drop branches near the existing `tx_too_large` handling)
- tests: `crates/shamir-server/src/tests/tx_registry_tests.rs` and a NEW topic file next to the existing db_handler tests for the handler behaviour (+ one-line `pub mod` in the `mod.rs`)
- NO engine files (`crates/shamir-engine/**` belongs to the ExecScope refactor running in parallel: do not edit `query/batch/*`), NO shamir-query-types changes, NO config.rs changes

## Task

W7 — TxExecute lifecycle on the server (cluster #1207; GitHub issue #46 `gh issue view 46`). The engine already bounds a TxExecute by a real deadline (PR #83); the SERVER does not yet bound a call by the transaction's remaining lifetime nor drop the transaction on a budget error.

Observed on master (verify): `InteractiveTx::new(.., max_lifetime, ..)` stores an absolute deadline (`deadline_nanos`, `tx_registry.rs` ~:117-140) but `TxExecute` does not consult it: a transaction created with `max_lifetime = 0` still executes. `tx_handlers.rs` (~:188-200) drops the transaction on `tx_too_large`; it does not do so on `ExecutionTimedOut` / `BatchError::LimitExceeded`.

Required result:
1. `InteractiveTx::remaining_lifetime() -> Duration` (zero when expired, saturating, monotonic clock the registry already uses).
2. In `TxExecute`: if the lifetime is already expired -> remove the handle and return the wire error `tx_expired` (add the code string next to the existing tx error codes; document it in the protocol/error list where `tx_too_large` is documented). Otherwise the per-call execution time is `min(server cap for TxExecute, remaining_lifetime)` — the request's own `max_execution_time_secs` can only lower it further (clamp, never raise). Reuse the existing clamp helper; do not duplicate it.
3. When the engine returns `ExecutionTimedOut` or `LimitExceeded` for a TxExecute, drop the transaction exactly like `tx_too_large` does today (safe: an interactive tx is snapshot/serializable only, see `db_tx.rs` ~:80-83; the staged writes were never committed). Never drop it on ordinary query errors.
4. Never add any time check around `TxCommit` or after `commit_tx` started — the budget applies to TxExecute only.
Red tests first: a tx built with `max_lifetime = 0` then TxExecute must fail with `tx_expired` (today it runs) and the handle is gone; a tx with ample lifetime and a tiny call budget gets `ExecutionTimedOut`-mapped error and is gone; a normal query error keeps the tx usable. Use the existing registry/handler fixtures (read `tx_registry_tests.rs` `make_tx`); no sleeps — drive time with the injected lifetime values.
Optional (only if it is a clean small change): W7b — reserve a RI-15 slot for TxExecute; otherwise leave it and say so.
