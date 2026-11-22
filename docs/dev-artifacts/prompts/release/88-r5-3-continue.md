# Brief: R5 step 3, CONTINUATION — finish the administrative shell's state and narrow data views (same branch, same PR)

Re-open every path and line before relying on it (lines drift). The previous executor session of this task ran out of its 150-minute limit and ended WITHOUT a report (it was in the middle of a turn with a worker still active): you do not know what it finished, what it left half done, or whether its tree compiles. Its edits are in this worktree, uncommitted (about 30 changed files, +730/-1850 lines because of moves, plus new files and folders such as `shell/parser/` (the split of the deleted `shell/parser.rs`), `shell/commands/state.rs`, `tests/admin/shell/secure/` (the moved `refusals.rs` and `secure_fixture.rs`), `tests/admin/shell/state_data.rs`, `src/engine/db_handler/repo_admin/operator_state.rs`, `db_handler/tests/cursor/get_record_tests.rs`, `query-builder/src/wire/admin/operator_state.rs` and its test). Treat ALL of it as UNVERIFIED work in progress. Its stray local `target/` folder was already deleted by the orchestrator.

## What (this step only)

The scope is exactly the committed brief `docs/dev-artifacts/prompts/release/67-r5-3-status-and-data-views.md` (items 1-7, the "Layout prerequisites" and the "Do not" lists) and the settled design `docs/dev-artifacts/design/admin-shell.md` (PR3 row of section 9). Read both, then read `git status` and `git diff` of this worktree (nothing is committed beyond the brief), and:

1. For EACH of the seven items of brief 67 and for the layout prerequisites (parser split first; `shell/connect/profile.rs` untouched; folder entry counts; no appends to the capped test files) decide from the code alone (not from comments or test names): DONE (mechanism present and correct, test present), PARTIAL (say what is missing) or MISSING. Write this table first in your notes; the final report must contain it.
2. Finish every PARTIAL and MISSING item exactly as brief 67 specifies. Do not redesign what is DONE and correct; fix what is wrong. Every behavior has its CI-only test (you cannot run it: say in the report why each fails without the fix), registered in its `mod.rs`; no sleeps, no clock assumptions.
3. Look for half-edits: a function renamed in one caller and not another, a `use` left unused, a new file or folder not declared in its `mod.rs`, a test file not registered or moved without fixing its `super::` paths and imports, a deleted file whose items are not re-exported from the new split, an exhaustive `match` over `DbRequest`/`DbResponse` in the workspace that still lacks one of the two new variants (list every place you checked: server dispatch, the HMAC/policy tables, the token perimeter, the Rust client mapping, the TS and Node bindings only where Rust matches exhaustively), a guide paragraph describing a command the code does not have, a public item widened without need (list every new cross-crate item and why). Re-read each changed file for the whole class of error before running any gate.
4. Run the gates (below), fix what they report, and write the final report.

## Cargo discipline — the previous run broke it, do not repeat

Workers of the previous run started `cargo` WITHOUT the cargo wrapper into a local `target/` folder of this worktree (about 1.1 GB; the disk is 95% full). Every cargo command, from you or from any worker you start, goes through the wrapper given in the rules below, with the ABSOLUTE shared target directory exactly as written there, with `-j 1`, executed from inside THIS worktree. Never start cargo from any other folder, never with a relative `CARGO_TARGET_DIR`, never a second cargo while one of yours is running, never let a worker run cargo (tell workers explicitly: no cargo, no build; only the top executor runs `fmt`, `check`, `clippy`). If a cargo command prints `Blocking waiting for file lock` for more than a few minutes, say so in the report instead of waiting out the clock. If you find a local `target/` folder in this worktree, report it and do not use it.

## Time

This run's hard limit is 120 minutes (this overrides the 150 written in the rules below): at about 80 minutes stop adding scope, bring the tree to a compiling, formatted, clippy-clean state and write the report; at 100 minutes the report is written even if a gate is unfinished (say exactly which). Do not spawn more than two workers and give each a disjoint file set. If time runs short the priority is: a compiling, clippy-clean tree with the layout prerequisites honoured, then items 1-4 (status, `data get`, perimeter and client obligations, commands), then tests, then the guide; say exactly what is missing.

## Not in this step (report only)

* Anything of PR4-PR6 (rights, activity, backups, schedule, export), a change of the replication owner's code for provenance or sample age, a registry dependency.

## Do not

Weaken any existing refusal, add a dependency or a config key, grow `BatchOp` or the gate file past their caps, touch `shell/connect/profile.rs`, or widen an item beyond what a fix needs (list every widening). Never grow a file past 1000 lines (`wc -l` every file you touch), folders at most 7 entries (recount after staging: `tests/admin/shell/` was full before this step).

## Layout and observable acceptance

Acceptance: `fmt`, `check --all-targets` and `clippy -D warnings` exit 0 on every crate you touch (expected: `shamir-query-types`, `shamir-query-builder`, `shamir-server-security`, `shamir-server`, `shamir-client`, `shamir-db`; add dependents only if a public signature changed) and `python -I scripts/ci/check_layout.py` is clean on the changed paths; the final report contains the DONE/PARTIAL/MISSING table with evidence (file and line), per fix the mechanism, the regression test and why it fails without the fix, every widening, the list of exhaustive matches checked, and what the orchestrator must observe in CI.

## Layout and fixture lessons

Count every folder you add to with `ls` (many are at 7 entries: `python -I scripts/ci/check_layout.py` counts only tracked files, recheck after staging); at most 1000 lines per file, one primary item per file, `mod.rs` re-exports only (the rust-guard structure gate allows nothing else in a `mod.rs`: shared helpers go in a named file), tests in `tests/` folders, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`. Persistent stores, every `TempDir` alive for the whole test, close handles and wait for the server task to finish before reopening a directory (Windows holds Fjall locks until the dropped engine's background handles finish: reopen with a bounded retry on `Locked` only, like `init_waiting_for_locks` in the function-family crash tests), handshakes not sleeps, every await bounded by a generous timeout (at least 120 s: a hang detector, the Windows runner is up to 10x slower), a batch is at most 50 queries, per-session request rate limit 500 req/s (never poll in a tight loop: pace status polls at 20 ms or more), set `argon2_concurrent_max` and `auth_init_rate_per_second` to 64 in fixtures that open several sessions, debug-test stacks are small (call concrete handlers through `Box::pin`, run deep tests on a roomy thread, `on_big_stack`), assert on invariants and sets, `shamir-test-diag` marks for flake-prone tests.

## Rules that bind the change

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: if a cargo run is already in progress wait for its notification, never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<artefacts>/target-exec wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <logfile OUTSIDE the repository> 2>&1` (the wrapper serializes cargo across executors and the shared `CARGO_TARGET_DIR` keeps registry dependencies compiled once for all executors: never run `cargo clean`, never set another target dir; never `-j 2`; never piped through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper); every worker you spawn must be told the same, and no worker may run cargo at all: only you, the top executor, run `check`/`clippy`/`fmt` under these rules (workers edit files and read code only).
* Leave no scratch files in the tree and none outside it (edit only paths inside this worktree).
* Wrap-up: the hard limit is 150 minutes. At about 105 minutes (70%) stop adding scope, bring the change to a compiling, formatted, clippy-clean state and write the final report (what is done, what is missing, what each check printed); a run that hits the limit without a report is lost work.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits. (For file moves use a plain filesystem move and say so in the report;
the orchestrator stages them so git detects the renames.)

