# Brief — W7c TxExecute sub-second budget seam and handler tests

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

- `crates/shamir-server/src/db_handler/tx_handlers.rs` (the TxExecute budget clamp ~:140-155 and the error/removal branch ~:205-225)
- `crates/shamir-db/src/shamir_db/execute/db_tx.rs` and `db_execute.rs` (`tx_execute_with_policy` / `execute_with_policy`: only the new budget-seam parameter or policy field)
- `crates/shamir-engine/src/query/batch/exec_scope.rs`, `authorized.rs`, `batch_execute.rs` (only the Duration-granular deadline seam; `ExecPolicy` lives in shamir-query-types: edit `crates/shamir-query-types/src/batch/exec_policy.rs` ONLY if the seam must be a policy field, and say so)
- `crates/shamir-server/src/byte_budget.rs` ONLY for the optional RI-15 item below
- tests: NEW topic files in `crates/shamir-server/src/db_handler/tests/` and `crates/shamir-engine/src/query/batch/tests/` (+ one-line `pub mod`)
- docs: `docs/dev-artifacts/roadmap/PHASE_B_INTERACTIVE_TX.md` (lifecycle section only)
- NO commit/WAL/drainer/read-path files, NO wire-format changes to BatchRequest / BatchLimits (the seam must NOT add a field to the wire type)

## Task

W7c (task #1281, work-budget cluster #1207) — finish the TxExecute lifecycle left over from W7 (PR #86). Merged on master (your base): W2 ExecScope, W3 effective limits, W4 server policy (`ExecPolicy`, `*_with_policy`), W6 work counter, W8 byte budget (`ExecScope::charge_bytes`). Read `exec_scope.rs`, `authorized.rs` (`root_scope`) and `tx_handlers.rs` first.

1. Duration-granular call budget. Today the handler computes `remaining_secs = remaining.as_secs() + (subsec_nanos > 0)` (rounded UP) and clamps `batch.limits.max_execution_time_secs` to it: a call may overrun the handle lifetime by up to 1 s and a sub-second remaining lifetime cannot be expressed. Add an engine-internal seam that carries an exact `Duration` cap from the handler into the root `ExecScope` (for example an optional `deadline_cap: Option<Duration>` argument of `tx_execute_with_policy` / an `ExecPolicy`-adjacent value that `Authorized::root_scope` applies with `min`) — NOT a new field on the wire types `BatchRequest` / `BatchLimits`. The effective deadline = min(request budget, policy cap, handler remaining) at full Duration precision; keep `max_execution_time_secs` semantics for everything else. The embedded `execute` / `tx_execute` entry points are unchanged (no cap).
2. Deterministic handler tests (no wall-clock sleeps longer than a few ms; use an injected tiny `Duration` budget / a paused-time or hook-based approach, whatever the existing handler test fixtures in `crates/shamir-server/src/db_handler/tests/` support — read them first):
   a. TxExecute whose remaining lifetime / budget is tiny gets the mapped time-out error code AND the handle is removed from the registry (a follow-up TxExecute on the same handle gets `tx_not_found`).
   b. A `BatchError::LimitExceeded` from a TxExecute (for example `max_total_work` or the byte cap with `enforce_result_size`) removes the handle too. The test MUST fail if the removal branch in `tx_handlers.rs` (`ExecutionTimedOut | LimitExceeded` -> `tx_registry.remove`) is reverted — verify this by reading, state it in the report.
   c. A sub-second remaining lifetime is honoured exactly (the executor deadline is not rounded up to a full second): assert on the effective scope deadline through the seam, not on elapsed time.
3. Docs: add `tx_expired` and the exact-budget rule to the lifecycle section of `docs/dev-artifacts/roadmap/PHASE_B_INTERACTIVE_TX.md` (what the client sees, when the handle is dropped, which error codes drop the handle).
4. OPTIONAL (W7b, RI-15): read `crates/shamir-server/src/byte_budget.rs`; if a reserved slot so that TxExecute cannot be starved by the global byte budget is a small, local change (roughly < 40 lines plus a test), do it; otherwise skip it and say exactly why in the report. Do not widen the scope for it.
Do NOT run benches. This is not a hot path beyond one `min` at scope construction.
