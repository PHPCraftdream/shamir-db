# Brief: BACKUP-COPY-INTEGRITY (task #2006) — the live `server_meta` copy without a consistent point, case aliases in the inventory, the documented limit of an archive that vanishes before the copy

Path prefixes used below: `sync/` = `crates/server/shamir-sync/src/` (the online backup lives under `sync/.../online/`, re-open it: the files were split into `online/copy/` by BACKUP-TAILS-SYNC); `server/` = `crates/server/shamir-server/src/`. Re-open every path and line before relying on it: this branch starts from `origin/master` after BACKUP-TAILS-SYNC and BACKUP-TAILS-SERVER have LANDED. The defects below EXISTED on master and were found by the independent reviews of BACKUP-TAILS-SYNC (reports rv_bts_3 and rv_bts_4); each is confirmed unless the re-check shows it is already fixed (report which). Other executors work in parallel in other files: CANCEL-TAIL (cancellation and activity code, guide sections on cancellation), ENGINE-RESULT-TAILS (batch results, native `Execute`); the open shutdown branches own `server_handle.rs`, the replication supervisor, the token service and the launch rollback. Do not touch those areas.

## Why

1. P1: the live store `server_meta` is copied file by file OUTSIDE any seal (step E of the online copy; step B seals the engine's system repository, not the `ServerMetaStore`). If an LSM flush switches `current` to `vN+1` after the file listing of step E but before `current` is read, the copy holds the new `current` and the old `vN` with no extra file and no hash mismatch: step F (hashes, sizes, inventory, stamp, WAL metadata) succeeds, and Fjall fails to open the copy (`vN+1` is missing). Pins: lsm-tree `version/persist.rs` ~19-51 and `version/recovery.rs` ~12-45; fjall `recovery.rs` ~64-98; the server loads the store in `durable_stores.rs` ~114-117. Code: `server/boot/config/diagnostics/backup_online/adapter.rs` ~324-336, `online/protocol.rs` ~816-824, `copy_engine.rs` ~626-640 (re-open: paths moved).
2. P2: the inventory does not require a one-to-one match between the manifest paths and the files on disk (`server/backup/offline/v2_verify.rs` ~113-139, ~283-294, ~473-501 compare strings). On a case-INSENSITIVE receiver that keeps the first spelling, an archive `audit.log.<n>` (lower case) and a regular source file `AUDIT.LOG.<n>` merge into one file: both manifest paths hash correctly, a restore onto a case-SENSITIVE target loses a path (`restore/staged_listing.rs` ~30-42), and the audit-chain recovery sees only the exact lower-case name (`audit/recovery.rs` ~370-400, ~447-458): a hole in the chain or an anchor past the tail.
3. A known limit: an archive that vanished BEFORE the copy leaves nothing for the guard of the accepted round; the guard is not an audit-chain validator.

## What (fix in this order; for each item write the failing test first where a test is possible)

### A. Item 1
Choose between (a) a consistent capture (one seal or common point for `server_meta`, as the repositories have) and (b) a refusal BEFORE publication when a throw-away copy of `server_meta` does not open in Fjall and the anchor is not validated against the copied chain (do NOT open the final tree in place: Fjall recovery changes files). DEFAULT when (a) is not small and clearly safe: (b), "verify by opening". Do not weaken the durability of the published bytes, the self-verification or the order of shutdown. The CI test must switch `current` AFTER the listing while the old version is still retained (a check by the inventory alone passes this case and must not be accepted as the oracle). If forcing the switch needs a test seam, add it `cfg(test)` only.

### B. Item 2
Require an exact ONE-TO-ONE match of the manifest entries and the listed regular files and refuse a cross alias (two manifest paths that differ only by case, or a path whose on-disk spelling differs). Tests on a case-sensitive and on a case-insensitive receiver: the case-insensitive behaviour must be testable on any OS (model the receiver in the test instead of relying on the runner's file system; say how).

### C. Item 3
Documentation only: the guide already states it; make it exact ("before any pass left a copy in the receiver") and make sure no sentence claims that the guard validates the audit chain.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-sync`, `shamir-server`); `python -I scripts/ci/check_layout.py` is clean (the orchestrator runs it after staging; mind the 7-entry folders and the 1000-line files); the final report lists per item the mechanism with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, the items already fixed on master, and anything not done. If the work does not fit the time limit, stop at the end of a complete item (A, B, C are independent) and list the items not reached: do not leave a half-edited item.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/backup-copy-integrity wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_bci_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
