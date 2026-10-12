# Brief — TEST-SPEED make vr5_cofilter test cheap

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

- `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` (the test `vr5_cofilter_sees_staged_and_filters_residual` and its helpers only)
- A narrowly scoped test seam in `crates/shamir-engine/src/table/` ONLY IF needed to reach the co-filter path cheaply (for example making the two path-selection thresholds overridable for tests through a `cfg(test)` or doc-hidden setter); no behaviour change of the production defaults
- NOT in scope: any other test, the HNSW implementation, production thresholds, `bench-iters.txt`

## Task

TEST-SPEED (task #1335): `vr5_cofilter_sees_staged_and_filters_residual` takes about 90 s in CI. The remediation series is MERGED on master, your base (SEED-1 repository identity/epoch and the wire gap interval are in). Verify every claim you act on against the code first (line numbers may have drifted). Sibling work runs concurrently: FOLLOWER-DURABILITY (PR in CI) owns `apply_replicated.rs`, `follower_loop.rs`, `supervisor.rs`, `admin_replication.rs`, the recovery-marker/bookmark helpers and the gate-seeding region of `repo_instance.rs`; VIS-2 will own query-types wire/config, `db_handler` and commit paths: stay strictly inside your scope. Tests that park one commit and then await another INLINE hang (commit and non-tx writes wait for the completion watermark W): spawn the second operation, never raise a timeout. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, no builds/tests (CI only), imports at the top, one primary export per file, `mod.rs` re-exports only, tests under `tests/` dirs, builder-only queries in code (wire-format docs excepted).

FACTS (verify): the test in `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` builds about 20.7k rows to cross `PRE_FILTER_MAX_CANDIDATES` (4096, `read_exec.rs`) AND `CO_FILTER_MAX_SELECTIVITY` (0.20, `hnsw_adapter.rs`); the HNSW build in a debug profile (`index2_on_insert` per row) dominates the time. It passes on master but nextest prints SLOW (> 60 s) and the per-test kill is 180 s, so it will flake under load. Hangs and slow tests are bugs per the repository rules: never raise the timeout.

GOAL: make the test fast (target: a few seconds in CI) WITHOUT weakening what it proves: the planner selects the `co_filter` path, a staged 'red' record appears in the result, a staged 'blue' record is filtered out, and the residual filter is applied. Preferred approach: find out how the two thresholds are read and add the smallest test seam that lets this test lower them (so a few hundred rows suffice) while production defaults stay byte-for-byte identical; add one assertion that the seam-lowered run really took the `co_filter` path (the existing `path_label` check) and keep a separate guard test that the PRODUCTION default thresholds are unchanged (a constant check, not a build). If a seam is impossible, find another honest way to reach the same planner decision cheaply (for example bulk-loading or smaller HNSW parameters through an existing config) and explain why. Do not delete or `#[ignore]` the test. Update the stale explanatory comment in the test to match the new construction.
Hot path: none (test only; production defaults unchanged).
