# Brief: SYNC-BACKUP-COST (task #2004) - remove the avoidable cost of one scheduled backup run in shamir-sync without weakening durability or verification

Path prefixes used below: `sync/` = `crates/server/shamir-sync/src/`; `server/` = `crates/server/shamir-server/src/`; `engine/` = `crates/engine/shamir-engine/src/`; `types/` = `crates/query/shamir-query-types/src/`. Re-open every path and line before relying on it: this branch starts from `origin/master` after release-1 batch 2 (CANCEL-TAIL, BACKUP-COPY-INTEGRITY, ENGINE-RESULT-TAILS slice, SHUTDOWN-GUARDS) has LANDED, and the line numbers come from earlier revisions. Three executors work in parallel in other files: REL-R2d-P5 (backup schedule state in the BackupStatus reply: `server/boot/config/diagnostics/backup_online/` registry, admin_port, schedule, the BackupStatus op type and builder, the guide section on backups), SYNC-BACKUP-COST (`sync/sync/durability/backup/online/` copy engine and protocol), NESTED-OUTCOME-WRAPPERS (`engine/query/batch/` arms and executor, the typed outcome refusal in `server/engine/db_handler/repo_admin/execute.rs`). Do not touch the files of the other two. `docs/guide-docs/guide/07-operations.md` may be edited by two executors in different sections: change only the sentences named below, with minimal hunks.

## Why

One scheduled backup run of a tiny data directory costs about 18 s on Ubuntu and 50 s on Windows in CI. There is no fixed wait on the run path (the tick with run-on-start is immediate; the 8 s seal budget and the 15 s zero-lease wait are limits, not sleeps; the 5 x 100 ms retries happen only on a sharing violation; lock files are skipped). The cost is per file and per sync. The same protocol on a one-repository fixture measured 1.883 s on Ubuntu and 22.339 s on Windows in an old full CI run (about 12x on Windows for the same number of calls). Three candidates, all in `sync/sync/durability/backup/online/` (the paths below are from before the copy engine moved into `online/copy/`: grep for the function names):

* (A) the residual second full copy rewrites and fsyncs every non-repository file (`protocol.rs` near lines 793-795 calling the copy engine near 634-652 in the old layout; the directory fsync near 901-909 of `copy/copy_engine.rs`).
* (B) `self_verify` hashes every manifest entry, then `verify_writer_snapshot` hashes them a SECOND time (`protocol.rs` near 436-453): remove the first loop without weakening the check; prove by reading that the second pass is strictly stronger (same bytes, same comparison, detects at least what the first detects), and say what the first loop alone caught that the second does not, if anything.
* (C) the directory fsync runs even when the pass added no entry (Unix).

## What

For each candidate decide from the code (read it fully, include the Windows paths) whether the work is redundant, and remove it only when the proof is complete; otherwise keep it and write one sentence why. Never weaken: the durability of published bytes (every published file and directory entry still reaches stable storage before the manifest is written and before the backup is reported complete), the self-verification of the published tree, the shutdown and cancellation order, the pins and the seal protocol. Each change needs a test that fails without it, or an explicit "protective, no behavioural test possible" note with the reason (a cost-only change may be covered by an existing test that observes the counted operation: add a `cfg(test)` counter of hashed files or of fsync calls at the existing test seam if one exists, otherwise do not invent a new seam: report instead). Do not change the manifest format, any error code, any public item or the on-disk layout. If a candidate cannot be removed safely, the report says so with `path:line` evidence: that is an acceptable outcome for that candidate.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for `shamir-sync` and for `shamir-server` if touched; `python -I scripts/ci/check_layout.py` clean (the orchestrator runs it after staging; `protocol.rs` and `copy_engine.rs` may be near 1000 lines: do not grow them, move code out if needed); the report lists, per candidate, removed or kept, the `path:line` proof and the test or the reason for none.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/sync-backup-cost wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_sbc_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
