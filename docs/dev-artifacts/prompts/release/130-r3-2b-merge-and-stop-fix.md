# Brief: R3 step 2b (task #1950) - resolve the merge of current `master` into the online export branch, then fix the shutdown P1 of the review

Path prefixes used below: `sync/` = `crates/server/shamir-sync/src/`; `server/` = `crates/server/shamir-server/src/`; `engine/` = `crates/engine/shamir-engine/src/`; `types/` = `crates/query/shamir-query-types/src/`. Re-open every path and line before relying on it.

## Situation

This branch carries the online export job (R3 step 2b, brief `docs/dev-artifacts/prompts/release/56-r3-2b-online-export-job.md`) as two unpushed WIP commits on a base that is about five days old: the live export job is wired in source (`server/.../export_boot.rs`, `server/engine/db_handler/repo_admin/execute.rs`, `export_online/service.rs`, `gate.rs`, `reader.rs`), `cargo check --all-targets` was clean on the old base, no CI has ever run. The orchestrator started `git merge origin/master --no-commit`: the working tree is IN THE MIDDLE OF THAT MERGE with 53 conflict hunks in 21 files (`grep -rn '^<<<<<<<' crates docs scripts` lists them). The side above `=======` is THIS branch (HEAD, the export work); the side below is `master`. You edit files only: you never run git; the orchestrator stages the result and commits the merge.

Three other executors work in parallel on the same master and overlap with some of your conflicted files: REL-R2d-P5 (schedule state in the BackupStatus reply: `server/boot/config/diagnostics/backup_online/registry.rs`, `admin_port.rs`, `schedule/`), SYNC-BACKUP-COST (`sync/sync/durability/backup/online/`), NESTED-OUTCOME-WRAPPERS (`engine/query/batch/` arms, the typed outcome path in `execute.rs`). Their work is NOT on your base; keep your hunks in the shared files minimal and local so the later merge of their branches stays mechanical.

## Why

Master moved a lot since the WIP: the R2d backup scheduler and registry, the R4b activity/cancel work (`execute.rs`, `tx_handlers.rs`, `session.rs`, the cursor and activity registries), shutdown batches 1 and 2 (`server_handle.rs`, the store release barrier, the replication supervisor), new batch operations in the shared enumerations. The earlier independent review of the WIP (`<artefacts>/rel-r3-2b3.run3.out`, read it first) returned FAIL with one production P1: on server stop the detached mutation workers are not awaited before the repository flush and `shutdown_clean` (`execute.rs` near 402-424, `tx_handlers.rs` near 469-478, `server_handle.rs` near 263-318 in the OLD revision), and the failure tests do not prove that the export freeze was never raised (`service_tests.rs` near 422-445).

## What (in this order)

### 1. Resolve every conflict, preserving both intents
Rules: for everything outside the online export, master wins and its behaviour must survive unchanged: the R2d registry and scheduler (`registry.rs`, 9 hunks: the export job must enrol into the registry the way master's code now works; read master's version first), the R4b activity/cancel/session code (`execute.rs`, `tx_handlers.rs`, `handler.rs`, `session.rs` in `shamir-connect` and in `engine-query`, `catalog_access.rs`), the shutdown code (`server_handle.rs`, `launch.rs`, `server_launcher/mod.rs`: master's ordering of the stop stages, the scheduled-worker join, the store release barrier and the panic-guarded replication wrapper stay verbatim; the branch has no business in `sync/.../supervisor/start.rs`: take master's side there entirely unless the branch changed something export related, in which case say what), the token matrix tests, the perimeter `scope.rs`, `hmac_tests.rs`. The shared enumerations (`types/ops/batch/ops/batch_op.rs`, `op_kind.rs`, `core/batch_op_classify.rs`, `op_kind_tests.rs`, `types/admin/activity/op_name.rs`, `types/admin/types/schema_migration/model/operation.rs`): keep BOTH sets of variants, tests and classifications; if the result pushes a file over 1000 lines move code into a new sibling file (one primary item per file, `mod.rs` re-exports only) instead of growing the file; a variant added on both sides with different names is a real decision: keep master's and adapt the export code. Test files: keep both sides' tests, adapt the export tests to master's fixtures and signatures. At the end `grep -rn '^<<<<<<<\|^=======\|^>>>>>>>' crates docs scripts tools` is empty and every file still parses.

### 2. Fix the P1 (shutdown vs detached mutation workers and the export freeze)
Read master's current stop ordering in `server/engine/server/server_handle.rs` and the tracked-task patterns it already uses. Required: a server stop (graceful, deadline, and the failure path of a started export) awaits, within the existing bounded-stage rules, every detached worker this branch spawns (the export job runner, mutation-source enrolment workers) BEFORE the repository flush and `shutdown_clean`; a timed-out wait is reported through the existing shutdown report, never silently dropped; the export freeze of a source is lifted exactly once on every exit path (success, failure, cancel, stop). No new unbounded wait, no new dependency, no new config key or metric. Name the stage in which each worker is joined and why the order is right.

### 3. Tests for the P1 and the freeze
Failure tests must prove the freeze was never raised on a refused start, and lifted exactly once after failure, cancel and stop (`service_tests.rs` near 422-445 in the old revision: re-open, strengthen). Add a test that a server stop with a running export job waits for the job and reports through the shutdown report. Every test names the mutant it kills (the line you would remove) and why it fails on the parent commit; every wait is bounded by at least 120 s; no sleeps for synchronization.

### 4. Gates
After your LAST edit: `cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` for every touched crate (expected: `shamir-query-types`, `shamir-connect`, `shamir-engine`, `shamir-engine-query`, `shamir-server`, `shamir-server-security`, `shamir-sync` if touched, `shamir-client`, `shamir-query-builder`). Write the first `check` only after the conflicts are resolved and the tests are written. The report is due at 70% of the time limit: ONE complete standalone message with every gate and its exit code printed after the last edit, the list of every conflicted file and which side won in each hunk class, and every deliberate deviation.

## Observable acceptance

No conflict marker left; every gate above exits 0; `python -I scripts/ci/check_layout.py` clean (the orchestrator runs it after staging; mind the 7-entry folders and the 1000-line files, especially `batch_op.rs`, `registry.rs`, `execute.rs`); no new `#[allow]`, no visibility widening of an existing item, no new dependency, no change of a wire tag or a stable error code of master.

## Do not

Change a wire tag or an existing stable error code; add a dependency, a config key or a metric; widen the visibility of an EXISTING item (run `python -I <orch>/vis_widen.py <worktree>` after your last edit and report its output; `<orch>` is the folder of the cargo wrapper `cg.sh` named in the rules below; the CI gate `structure` rule `VIS_WIDENING` has no exemption for tests); weaken or delete a test; use sleeps for synchronization; add `#[allow]`; touch the areas of the other executors and of the open shutdown branches named at the top.

## Review lessons (apply before you report; every one of these defect classes was found by independent reviews of earlier steps)

* An oracle that cannot fail: for every new test name the mutant it kills (the code line you would remove or flip) and why the test fails on the parent commit. A test that passes without the fix is not a test of the fix.
* A fixture that clears its own fault, a negative control that cannot go red, a sleep used for synchronization (handshakes, injected clocks and bounded awaits only), an unbounded await in test setup, a `TempDir` that is dropped before the test ends.
* A statement in the guide, a design text or a comment that exceeds what the code does: every sentence you write or leave must match the code you can point to by `path:line`.
* A test or a mechanism cited in the report that does not exist: before the final report grep EVERY test name you cite, in the changed files, and print the grep result per name.
* The rust-guard `structure` gate fails the change when the visibility of an EXISTING item is widened (new items are fine) and on any new `#[allow]`, inline `#[cfg(test)]` module, local `use` or logic in a `mod.rs`: avoid all of them; list every widening and every new cross-module item in the report.
* A finding fixed at one site while its sibling sites keep the defect: for each fix grep the whole class (same helper, same pattern, same file family) and either fix every site or say why a site is different.

## Layout and fixture lessons

Count every folder you add to with `ls` (7-entry limit; `python -I scripts/ci/check_layout.py` counts only tracked files, recheck after staging); at most 1000 lines per file (`wc -l` every file you extend; put new code in new files); one primary item per file, `mod.rs` re-exports only, tests in `tests/` folders, no inline `#[cfg(test)]` modules, no local `use`. Every `TempDir` alive for the whole test, handshakes not sleeps, every await bounded by a generous timeout (at least 120 s: the Windows runner is up to 10x slower), assert on invariants, `shamir-test-diag` marks for flake-prone tests. Unix-only code is compiled only in CI and Windows-only code only on this machine: every `cfg(unix)`/`cfg(windows)` item gets its cfg on the item AND its imports and helpers.

## Rules that bind the change

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/rel-r3-2b wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_r3b_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
* Leave no scratch files in the tree or outside it. No private machine paths in any file you write.
* Wrap-up: the hard limit is 150 minutes. At about 110 minutes stop adding scope, bring the change to a formatted, check-clean and clippy-clean state and write the final report: ONE complete standalone message (not a pointer to an earlier report) with every gate and its exit code printed AFTER your last edit. A run that hits the limit without that report is lost work.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits. (For file moves use a plain filesystem move and say so in the report;
the orchestrator stages them so git detects the renames.)
