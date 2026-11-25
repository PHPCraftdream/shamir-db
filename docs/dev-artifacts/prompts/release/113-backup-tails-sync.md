# Brief: BACKUP-TAILS-SYNC — the open items of the online-backup protocol of `shamir-sync` (audit-file rotation during a copy, the missing tests of the unstable audit phase, the fixtures, the manifest-publication fsync tolerance)

Path prefixes used below: `online/` = `crates/server/shamir-sync/src/sync/durability/backup/online/`; `tests/` = `online/tests/`. Re-open every path and line before relying on it (this branch starts from `origin/master` after R2d step B (#646) has LANDED; other executors work in parallel in other files: the shamir-server side of the backup scheduler tails (`crates/server/shamir-server`: scheduler, registry, retention, metrics, guide), R4b-4 shape statistics, R5-5 shell maintenance commands, interactive-transaction locks, SERVER-HARDENING, FENCE-TAILS: do not touch their areas; if you must touch a file outside `shamir-sync`, say so and keep it minimal).

## Why

The independent reviews of the online backup (the audit-chain copy, R2d step A and step B) left CONFIRMED defects and missing tests that were deliberately not fixed at landing. Each item below is a confirmed finding: fix it, do not defer it. The audit-file rotation item is a real production defect (a copy fails with a spurious error when the audit log rotates at an unlucky instant).

## What (fix in this order; for each item write the failing test first where a test is possible)

1. **Rotation during a corrective re-copy (P2, pre-existing).** A rotation that renames `audit.log` exactly at a corrective re-copy or at its follow-up hash fails the copy with `Io NotFound`: `online/copy_engine.rs` `run_copy_pass_inner`'s re-copy loop propagates `NotFound`, while the FIRST copy path counts a vanished source as missing. In the audit-only mode treat a vanished source as MISSING and let the stability check re-list. Test: an injected rotation between the first copy and the corrective re-copy (use the existing rotation fixture and seams of `tests/failure_injection_tests.rs`); the copy must succeed and the manifest must list the new archive. Name the mutant (propagating `NotFound` again).
2. **No test drives an unstable phase-2 round or the four-round `AuditFilesUnstable` failure.** Plan: an `after_capture` hook that creates plain fake archives `audit.log.<20 digits>` in the source (no appender needed): limit 1 -> the second round succeeds and the manifest lists the new archive; unlimited -> `AuditFilesUnstable { rounds: 4 }`. This needs a layout split of `tests/` first: the folder has 7 entries, `copy/copy_seam_tests.rs` is about 996 lines and `failure_injection_tests.rs` about 962: new tests go into a NEW subfolder/file; count with `ls`, `wc -l`.
3. **Recovery fixtures are too weak.** They use fixed keys and no `server_meta` anchor and no `server_meta` store, so a mutant that drops `server_meta` from phase 1 passes; the rotation fixture header says both anchors are persisted. Make the fixtures persist both anchors and the keys vary; add the assertion that fails for the mutant "drop server_meta from phase 1".
4. **`list_audit_files` discards per-entry enumeration and metadata errors** (an injected metadata error hides a new archive from BOTH listings). Make an unreadable entry a typed error (do not echo a path into a user-visible message: the vocabulary of the copy errors is path-free). Test with the metadata-error seam.
5. **Test hygiene.** The rendezvous channel waits of the rotation test (`failure_injection_tests.rs`) ignore the release result; `JoinOnDrop` joins unbounded. Bound every await and join with its own timeout well under the 180 s kill, and check the results.
6. **`recreate_fjall_locks` fixture fingerprint** (version + keyspaces) is looser than production (the FJL header); only `audit_checkpoint` is exercised: tighten the fingerprint to the production one, or state precisely what the fixture cannot reproduce.
7. **Manifest publication versus directory fsync (from the R2d step B review, P3/D).** A filesystem that cannot fsync a directory is tolerated by the copy pass (`copy_engine.rs` `fsync_dir_fallible` through `manifest::fsync_dest_dir`, which tolerates `Unsupported` and `InvalidInput` through `manifest::tolerate_dir_sync_capability`), but `write_manifest` and `write_backup_meta` (`online/manifest.rs`) still call the private non-tolerant `fsync_dir`: on such a mount the copy passes succeed and the run fails as `verifying: destination-io-unsupported`. Bring every directory fsync of the destination to ONE rule (tolerate exactly those kinds, everywhere), correct the doc of `fsync_dest_dir`, and test it through the existing seam (the injected result must pass through the same production post-processing as the real call: that is the lesson of the step B round 3; check every other seam that substitutes a result for the same class).
8. **Small items.** `CopyError::AuditFilesUnstable` is new public surface of a `publish = false` crate: say so in the doc; the new `pub(crate)` items of `audit_chain_files.rs` are counted by the rust-guard structure rule if a campaign ever covers `shamir-sync`: keep them as narrow as the callers allow; the adjacent online-status text of `docs/guide-docs/guide/07-operations.md` around the Preparing/Copying hints is pre-existing and imprecise: re-read it against the code and correct it.

## Do not

Change a wire tag, a stable error code or the copy protocol's behaviour beyond the items above; add a dependency, a config key or a metric; widen the visibility of an EXISTING item (the CI gate `structure` rule `VIS_WIDENING` has no exemption for test files: give a new helper its own new file instead of widening an old one; run `python -I <orch>/vis_widen.py <worktree> (<orch> is the folder of the cargo wrapper `cg.sh` named in the rules below)` after your last edit and report its output); weaken or delete a test; use sleeps for synchronization; add `#[allow]`; touch the scheduler, the registry, the retention code or the metrics of `shamir-server`.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for `shamir-sync` (and any other touched crate); `python -I scripts/ci/check_layout.py` is clean (I run it after staging); the final report lists per item 1 to 8 the mechanism with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, and anything not done.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/rel-backup-tails-sync wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_bts_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
