# Brief: ENGINE-RESULT-TAILS (a slice of task #2007, items 14, 15, 16) — a busy refusal under a tiny result limit, the lost synchronous-flush warning of a transactional `Batch` child, the missing `ForEach` late-abort test

Path prefixes used below: `engine/` = `crates/engine/shamir-engine/src/`; `server/` = `crates/server/shamir-server/src/`. Re-open every path and line before relying on it: this branch starts from `origin/master` after ENGINE-PATH-TAILS has LANDED. The items were left out of that landing by the independent review rv_ept_4 (documented, not fixed); each is confirmed unless the re-check shows it is already fixed (report which). Other executors work in parallel in other files: CANCEL-TAIL (cancellation and activity code), BACKUP-COPY-INTEGRITY (online backup copy, offline verify and restore); the open shutdown branches own `server_handle.rs`, the replication supervisor, the token service and the launch rollback. Do not touch those areas; the rest of task #2007 (items 1 to 13 and 17) is NOT in this slice: do not start it.

## Why and What (fix in this order; for each item write the failing test first where a test is possible)

1. P1 `server_busy` under a tiny result limit. A native `CreateUser` that is refused with `server_busy` (the Argon2 gate) under a client `max_result_size` of 1 byte still becomes `result_too_large`: the exception in `server/engine/db_handler/repo_admin/execute.rs` ~426-473 covers only the canonical `unknown_table` answer. Classify by the TYPED reason (not by text), return a bounded server text for the refusal, and test with a 1-byte limit and a permit holder (the way the Argon2 gate tests hold the permit; re-use their fixture).
2. P1 a transactional `Batch` child loses the synchronous-flush warning at the boundary of `run_sub_batch_arm` (`engine/query/batch/exec/.../arms_read.rs` ~154-181; re-open the path): carry the final disposition and the warning across the result boundary, and do not stamp a final receipt before the flush verdict is known. Test with an injected flush failure.
3. P1 `ForEach` and an uncertain outcome. The attempted version is lost when the outcome is uncertain: use the typed `BatchError::WalOutcomeUnknown` (check what is already done: after a committed prefix, a late abort no longer returns a repeatable code: `transaction_aborted` instead of `tx_conflict`, the reason in the text). The test "a prefix is committed, a late iteration fails with `tx_conflict`" is still missing: write it, and fix whatever it shows.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-engine`, `shamir-server`, plus any crate whose public type you extend: list them); `python -I scripts/ci/check_layout.py` is clean (the orchestrator runs it after staging); the final report lists per item 1 to 3 the mechanism with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, the items already fixed on master, and anything not done. Respect the layer rule of `shamir-engine` (`repo < table < validator < tx < query < instance < db_instance`; `.rust-guard/layers.toml`; `.rust-guard/layers_baseline.toml` must not change). If the work does not fit the time limit, stop at the end of a complete item and list the items not reached: do not leave a half-edited item.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/engine-result-tails wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_ert_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
