# Brief — W8 runtime byte budget

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

- `crates/shamir-engine/src/query/batch/exec_scope.rs` (the `bytes` counter methods and the `enforce_result_size` flag), `crates/shamir-engine/src/query/batch/batch_execute.rs` and `query_runner.rs` ONLY where a result is inserted into the results map and where ForEach / nested Batch containers are assembled; NEW file `crates/shamir-engine/src/query/batch/result_size.rs` (the iterative size estimator, one primary export)
- `crates/shamir-engine/src/query/batch/authorized.rs` (read `policy.enforce_result_size` into the root scope)
- tests: NEW topic files in `crates/shamir-engine/src/query/batch/tests/` (+ one-line `pub mod`)
- NO server files (W4 wiring is in a separate open PR; `ExecPolicy.enforce_result_size` already exists), NO shamir-query-types changes, NO tx/WAL files

## Task

W8 (task #1278; B12 / issue #45, work-budget cluster #1207) — a byte budget at RUNTIME for batch results. Merged on master (your base): W2 (`ExecScope { limits, deadline, work, bytes }`, `Authorized::policy`), W3 (effective limits incl. `max_result_size`), W6 (runtime work counter: read it first, mirror its structure for bytes). Today `max_result_size` is never enforced at runtime for batch results: intermediate results (`return_result = false`) and ForEach / nested Batch containers are built without any size accounting.

Why only when `ExecPolicy.enforce_result_size`: the embedded default max_result_size is 10 MB and enforcing it unconditionally would break embedded users and the WASM `db_gateway` reads; the server policy (W4) turns the flag on.

Required result:
1. `result_size.rs`: an ITERATIVE (explicit stack, no recursion: deep values must not blow the stack) size estimator for `QueryValue` / `QueryResult` (bytes of materialized data: string and binary lengths, a fixed per-element overhead for containers, per-record overhead; document the formula in one comment and keep it conservative and deterministic). Unit tests with exact numbers for scalars, nested maps/lists, binary, empty containers.
2. `ExecScope::charge_bytes(n) -> Result<(), BatchError>` (relaxed `fetch_add`, `LimitExceeded { kind: ResultBytes, actual, max: limits.max_result_size }` when over the cap) — a no-op returning Ok when the scope was built with `enforce_result_size == false` (the flag comes from the `Authorized` policy into the root scope). No give-back accounting (documented: the budget is "bytes materialized by this execution").
3. Charge when a result is inserted into the results map (`all_results`) INCLUDING intermediate results with `return_result = false`, and for ForEach / nested Batch containers (charge the container as it is assembled, per iteration result, so an over-budget loop stops early rather than after building everything).
4. The check stays before `commit_tx` like every other budget; nothing in or after the commit.
5. HOT PATH: one extra pass over each result when enforcement is ON; when OFF the cost is a single predictable branch (a bool read) per insert — no estimator call. Do NOT run benches (state that `select_pipeline` and `read_path_matrix` are owed).
Red tests: an intermediate result with `return_result = false` plus a ForEach whose total is above a small cap -> `LimitExceeded { ResultBytes }` (wire code `result_too_large` via the existing mapping); the same batch through the embedded `execute()` (flag off) still passes unchanged; an over-budget loop stops before building every iteration (assert the number of executed iterations); a batch exactly at the cap passes (boundary); nested containers are charged once (no double counting between a nested Batch result and the entries it contains — assert exact byte totals with the estimator).
