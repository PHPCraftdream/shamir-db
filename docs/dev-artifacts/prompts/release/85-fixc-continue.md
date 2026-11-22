# Brief: R2d-FIX-C follow-up, CONTINUATION — finish the review fixes of the snapshot read lease (same branch, same PR)

Re-open every path and line before relying on it (lines drift). The previous executor session of this task ran out of its 150-minute limit and ended WITHOUT a report: you do not know what it finished, what it left half done, or whether its tree compiles. Its edits are in this worktree, uncommitted (23 changed files, about +1550/-450 lines, plus new files: `progress_attribution.rs`, `schedule/destination.rs`, `writer_lease_tests.rs`, `snapshot_lease_tests.rs`, `tagged_export_tests.rs`). Treat all of it as UNVERIFIED work in progress.

## What (this step only)

The scope is exactly the committed brief `docs/dev-artifacts/prompts/release/82-fixc-review-fixes.md` (items 1-7: unleased read of a snapshot absent at the first lookup; marker provisioning failure not counted as a retention failure and untagged snapshots; tagging racing a leased verifier; readers must never create the marker and read-only media; the unleased verification API and the thread-scoped bypass; staging path in `StagedFileMissing` and the legacy CLI refusing after modifying the target; the Russian guide). Read that brief (its appendix is the reviewer's full report), then read `git diff origin/master...HEAD` and `git status`, and:

1. For EACH of the seven items decide from the code alone (not from comments or test names): DONE (mechanism present and correct, test present), PARTIAL (say what is missing) or MISSING. Write this table first in your notes; the final report must contain it.
2. Finish every PARTIAL and MISSING item exactly as brief 82 specifies. Do not redesign what is DONE and correct; fix what is wrong. Every fix has a regression test (CI-only: you cannot run it, so say in the report why it fails without the fix) registered in its `tests/mod.rs`; no sleeps, no clock assumptions.
3. Look for half-edits: a function renamed in one caller and not another, a `use` left unused, a new file not declared in its `mod.rs`, a test file not registered, a guide paragraph describing a mechanism the code does not have, a public item widened without need (list every new cross-crate item and why). Re-read each changed file for the whole class of error before running any gate.
4. Run the gates (below), fix what they report, and write the final report.

## Cargo discipline — the previous run broke it, do not repeat

A worker of the previous run started `cargo clippy` WITHOUT the cargo wrapper and with a RELATIVE target directory; it created an untracked `target-exec/` folder in the main checkout, sat blocked on a lock for 40 minutes and burned the run. Every cargo command, from you or from any worker you start, goes through the wrapper given in the rules below, with the ABSOLUTE shared target directory exactly as written there, executed from inside THIS worktree. Never start cargo from any other folder, never with a relative `CARGO_TARGET_DIR`, never a second cargo while one of yours is running, never let a worker run cargo (tell workers explicitly: no cargo, no build; only the top executor runs `fmt`, `check`, `clippy`). If a cargo command prints `Blocking waiting for file lock` for more than a few minutes, say so in the report instead of waiting out the clock.

## Time

This run's hard limit is 120 minutes (this overrides the 150 written in the rules below): at about 80 minutes stop adding scope, bring the tree to a compiling, formatted, clippy-clean state and write the report; at 100 minutes the report is written even if a gate is unfinished (say exactly which). Do not spawn more than two workers and give each a disjoint file set.

## Not in this step (report only)

* Finding 7 of the reviewer (Windows case collisions in staged names): one sentence in the guide, nothing else.
* The migration gate re-reading the source manifest unleased after `verify_manifest`: mention in the report.
* A same-account operator unlinking a held marker on Unix: one sentence in the guide.
* Retention selection rules, ownership matching and the budget belong to the remainder task: do not touch them.

## Do not

Touch the audit code, the shutdown path or the online copy engine, add a dependency or a config key, weaken any existing refusal, or widen an item beyond what a fix needs (list every widening). `registry.rs` was 986 lines: re-measure every file you touch with `wc -l`, never grow a file past 1000 lines, move code into a named sibling file instead; folders at most 7 entries (recount after staging).

## Layout and observable acceptance

Acceptance: `fmt`, `check --all-targets` and `clippy -D warnings` exit 0 on `shamir-sync` and `shamir-server`; the final report contains the seven-row DONE/PARTIAL/MISSING table with evidence (file and line), per fix the mechanism, the regression test and why it fails without the fix, every widening, and what the orchestrator must observe in CI.

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

