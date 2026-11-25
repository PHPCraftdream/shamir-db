# Brief: NESTED-OUTCOME-WRAPPERS (task #2019) - nested Batch and ForEach lose the typed server_busy and WalOutcomeUnknown, and a transactional Batch child hides an aborted or indeterminate commit

Path prefixes used below: `sync/` = `crates/server/shamir-sync/src/`; `server/` = `crates/server/shamir-server/src/`; `engine/` = `crates/engine/shamir-engine/src/`; `types/` = `crates/query/shamir-query-types/src/`. Re-open every path and line before relying on it: this branch starts from `origin/master` after release-1 batch 2 (CANCEL-TAIL, BACKUP-COPY-INTEGRITY, ENGINE-RESULT-TAILS slice, SHUTDOWN-GUARDS) has LANDED, and the line numbers come from earlier revisions. Three executors work in parallel in other files: REL-R2d-P5 (backup schedule state in the BackupStatus reply: `server/boot/config/diagnostics/backup_online/` registry, admin_port, schedule, the BackupStatus op type and builder, the guide section on backups), SYNC-BACKUP-COST (`sync/sync/durability/backup/online/` copy engine and protocol), NESTED-OUTCOME-WRAPPERS (`engine/query/batch/` arms and executor, the typed outcome refusal in `server/engine/db_handler/repo_admin/execute.rs`). Do not touch the files of the other two. `docs/guide-docs/guide/07-operations.md` may be edited by two executors in different sections: change only the sentences named below, with minimal hunks.

## Why

Pre-existing gaps found by the independent review of ENGINE-RESULT-TAILS (documented as known limits in that PR body and in `docs/guide-docs/guide/07-operations.md`, item 4 of the result-limit text). Re-check each; fix it unless it is already fixed.

## What (in this order; failing test first where a test is possible)

1. A `server_busy` refusal of `CreateUser` inside a NON-transactional Batch child or a ForEach body gets a non-empty alias from the wrappers (`engine/query/batch/` `arms_read.rs`, near 167-179, 387-390 and 484-496 in the earlier revision) and, under a tiny client `max_result_size`, becomes `result_too_large`; with the engine byte meter on, ForEach charges the result shell before iteration 0. Fix: pass the typed empty-alias refusal through the wrappers when there is no committed prefix; keep byte-limit failures distinct. Add real-port coverage (the existing tests of the landed slice show the pattern: grep the tests of `server_busy` under a tiny limit in `engine/query/batch/tests/` and in `server/engine/db_handler/tests/`) for BOTH containers.
2. A transactional Batch child whose commit is aborted or indeterminate returns `Ok((empty map, Some(info)))` and `run_sub_batch_arm` keeps only `.results` (near 155-181): the parent can report success with an empty child value although the write may have committed. Decide the code (the parent has no committed-prefix knowledge: `transaction_aborted` with the cause in the text, or the abort code; keep the landed ForEach rule: after a committed prefix the code is `transaction_aborted` with `abort code {code}` in the text, never the retryable `tx_conflict`) and surface the reopen-required error with the attempted version for the indeterminate case. State the decision and the reason in one paragraph of the report and in the guide item.
3. `WalOutcomeUnknown` is rebuilt as a `QueryError` by the outer wrappers (the typed version is lost, code and text kept); with the byte meter off and a tiny cap the code can become `result_too_large`. Fix: keep the typed outcome through the wrappers, rendered by bounded server-owned code inside reserved headroom (no new limit key, no new metric).
4. Test strength of the landed ForEach indeterminate test: assert the exact attempted version and that no later iteration was dispatched; every wait bounded (at least 120 s).

Update only the sentences of the guide that these fixes make false (the known-limit sentence of item 4 and the nested-container statements); every sentence must match the code by `path:line`.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-engine`, `shamir-server`, possibly `shamir-db`); `python -I scripts/ci/check_layout.py` clean (the orchestrator runs it after staging; `arms_read.rs` is large: put new code in new files); every new test is named in the report with the mutant it kills and why it fails on the parent commit; no wire tag or stable error code changed (a new use of an existing code is fine, say where), no `#[allow]`, no visibility widening of an existing item, no new dependency.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/nested-outcome-wrappers wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_now_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
