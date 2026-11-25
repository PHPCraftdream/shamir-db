# Brief: REL-R2d-P5 (task #1978) - a typed read of the backup schedule state for superusers, no write

Path prefixes used below: `sync/` = `crates/server/shamir-sync/src/`; `server/` = `crates/server/shamir-server/src/`; `engine/` = `crates/engine/shamir-engine/src/`; `types/` = `crates/query/shamir-query-types/src/`. Re-open every path and line before relying on it: this branch starts from `origin/master` after release-1 batch 2 (CANCEL-TAIL, BACKUP-COPY-INTEGRITY, ENGINE-RESULT-TAILS slice, SHUTDOWN-GUARDS) has LANDED, and the line numbers come from earlier revisions. Three executors work in parallel in other files: REL-R2d-P5 (backup schedule state in the BackupStatus reply: `server/boot/config/diagnostics/backup_online/` registry, admin_port, schedule, the BackupStatus op type and builder, the guide section on backups), SYNC-BACKUP-COST (`sync/sync/durability/backup/online/` copy engine and protocol), NESTED-OUTCOME-WRAPPERS (`engine/query/batch/` arms and executor, the typed outcome refusal in `server/engine/db_handler/repo_admin/execute.rs`). Do not touch the files of the other two. `docs/guide-docs/guide/07-operations.md` may be edited by two executors in different sections: change only the sentences named below, with minimal hunks.

## Why

The admin shell design (`docs/dev-artifacts/design/admin-shell.md`, the table near line 331 and the row P5 near line 373) requires a read-only `backup schedule show` fed by a typed schedule state that the R2d work was supposed to land and did not (the R2d brief forbade new admin ops and wire types). Today the schedule state is visible only in `/metrics` (`git grep` for `ScheduleStatus` and `schedule_status` over `crates` is empty). Without it the shell reports `backup schedule show` as unsupported (exit 6) and the REL-R5-6 task (#1971) cannot finish.

## What

Default variant (keep it unless the code makes it impossible; if you deviate say why): do NOT add a `BatchOp` variant (`types/ops/batch/ops/batch_op.rs` is at its size limit). Add an OPTIONAL `schedule` section to the reply of the EXISTING `BackupStatus` operation (`types/admin/types/ddl/migration_ops.rs` `BackupStatusOp`; the reply is built in `server/boot/config/diagnostics/backup_online/admin_port.rs` `snapshot_value` from `registry.rs` `BackupStatusSnapshot`; the dispatch and the follower refusal are in `server/engine/db_handler/handlers/backup_handlers.rs`). The Rust query builder and the Rust client get NO new methods (the reply is a map; document the new keys). The section carries, as typed values read from the scheduler state that already exists (`server/boot/config/diagnostics/backup_online/schedule/` `config.rs`, `runner.rs`, `retention*`, `metrics.rs`; reuse the values the runner and the metrics already keep, never recompute or re-read the clock for a new measurement): whether the schedule is enabled, the interval, the window if the config has one, the retention policy, the time and the result of the last run (success, failed with the stable stage and reason codes, cancelled), the next planned run, and the run counters that the metrics already expose. Absent schedule (disabled) means an explicit `enabled = false` section or no section: decide, state it in the guide, and test it. Read-only, no write, no new lock held across an await. Authorization: the section is superuser-only; read how `BackupStatus` is authorized today (`session.rs` / the admin policy for `BackupStatus` in `engine-query`) and keep the rule at least as strict; a non-superuser must not receive the section (and must not learn that a schedule exists beyond what `BackupStatus` already reveals). Token sessions must behave exactly as they do for `BackupStatus` today (test the observed behaviour, do not invent a new rule). A follower refuses exactly as `BackupStatus` does.

Tests (CI-only; put them in the folder of the existing backup dispatch tests `server/engine/db_handler/tests/policy/backup_online/`, new files, mind the 7-entry limit): schedule enabled with a completed run (every key and its value type), schedule disabled, a non-superuser, a token session, a failed run (stage and reason are the stable codes). Name the mutant each test kills (for example: the section built from a second clock read; the section visible to a non-superuser).

Documentation: the backup section of `docs/guide-docs/guide/07-operations.md` (find the BackupStatus / scheduled backups text; the design cites section 8 of the release guide): one short subsection that lists the keys, their types and when they are absent. Every sentence must match the code you can point to by `path:line`. No change to `admin-shell.md` unless a statement there becomes false.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-query-types`, `shamir-server`, possibly `shamir-query-builder`); `python -I scripts/ci/check_layout.py` is clean (the orchestrator runs it after staging); every new test is named in the report with the mutant it kills and why it fails on the parent commit; no new `#[allow]`, no visibility widening of an existing item, no new dependency, no new metric.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/rel-p5-schedule-state wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_p5_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
