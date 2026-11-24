# Brief: CANCEL-TAIL (task #2005) — the tail of the request and transaction cancellation work: tests for three behaviours fixed by hand, the races that stayed open, the server-side counters, the guide

Path prefixes used below: `server/` = `crates/server/shamir-server/src/`. Re-open every path and line before relying on it: this branch starts from `origin/master` after TX-LOCK-LEAK (#2001) and R4b-4 have LANDED, and several items below may already be fixed by them (report which). Other executors work in parallel in other files: BACKUP-COPY-INTEGRITY (online backup copy and offline verify/restore in `shamir-sync` and `shamir-server`), ENGINE-RESULT-TAILS (batch results and the native `Execute` result limit in `shamir-engine` and `server/engine/db_handler/repo_admin/execute.rs`); the open shutdown branches own `server_handle.rs`, the replication supervisor, the token service and the launch rollback. Do not touch those areas; if you must touch a file outside the cancellation and activity code, say so and keep it minimal.

## Why

The reviews of the request/transaction cancellation work (R4b step 3: an executor round-2 review and the independent review rv-r4b3-1, verdict FAIL with two P1 on its scale) left a tail. The orchestrator fixed three behaviours by hand without tests: (a) the audit scope of a cancel (only refusals and a successful cross-user cancel are audited; a foreign `already_finished` is not), (b) the busy path: a context that is Gone or replaced answers `already_finished`, (c) `TxExecute` with a taken context answers the typed `tx_not_found`. The same reviews found races and test weaknesses that stayed open. Each item below is a confirmed finding unless the re-check shows it is already fixed: fix it, do not defer it.

## What (for each item write the failing test first where a test is possible)

### A. Tests for the three hand-fixed behaviours (the first and most valuable part)
1. A test per behaviour (a), (b), (c) above, each naming the mutant it kills. `cancellation_tests.rs` is at 949 lines: put the new tests in a NEW file of the same test folder, never in that file.

### B. Races and lifecycle holes (P2)
2. The control connection disconnects while a cancel's release awaits a record state mutex: the remaining locks of the transaction are left behind (the release in `cancel.rs` runs after the entry is removed; connection teardown aborts the dispatch tasks). Make the release survive the abort of its caller (a detached or guarded release, or an order in which the entry is removed last) and test it.
3. A cancel resolves the repository by its stored NAME, so a rename skips the release (check what TX-LOCK-LEAK already closed: it fixed release by the stored name after a rename; verify and keep only what is still open); a Committing transaction carries no identity, so a stale authorization can get `already_committing`; a cursor cancel authorizes mirror metadata and removes by ID (a trusted embedder can reuse an ID). Fix what is still open.

### C. Counters and the parked-cancel shell test
4. A server-side counter of DECODED server REQUESTS (not connections, not process starts) so that the shell test `parked_cancel` (`crates/server/shamir-server/tests/admin/shell/access_activity/parked_cancel*.rs`) can assert that exactly one effective cancel and then one extra request fail: use the existing activity or cancellation counter if one fits, otherwise add the smallest counter; extend the assertion in that test. Counter coverage to add: a successful cursor cancel, a busy `CancelTx`, a successful `CancelRequest`, and that a permission denial is NOT counted.

### D. Test weaknesses (P2)
5. The stale-identity handler half uses an id that was never present (use one that existed and was replaced); the commit race spawns two tasks without forcing their overlap (force it with handshakes); the parked reservation test never completes a commit; the counter test in `cancellation_tests.rs` spins with `yield_now` (not paced) and asserts before joining; the audit test ignores the result of `drain()` and its refusal case is cross-user only (add the same-user refusal); the `op_kind` test uses an empty map payload instead of a missing discriminator; a nested repeated key can collapse before the `BatchOp` visitor (decide by reading the decoder; test or document).

### E. Guide (`docs/guide-docs/`, the operations guide; re-read the sections on cancellation, activity and metrics) — every sentence must match the code
6. Second-connection claims (the 32-connection cap per native boot; an idle transaction or cursor can be cancelled from its own connection; the client documentation in `request.rs` ~769-773); `TxBegin` (`tx_open`) is not cancellable; handles are not secret (the owner receives them in `TxOpened`/`CursorPage`, so "only through activity" is false); state-row semantics (`active`/`awaiting_commit`/`expired_pending_reap` qualifications, a lease can refresh an idle-expired row); the audit queue limits (32 per principal, 256 global) and discard on drain expiry; `/metrics` needs observability and a working recorder; cursor metadata is stamped by `CreateCursor`.

### F. Design gap
7. `TxExecute` is registered non-cancellable, so "cancel the request first" is not executable (the guide says wait and retry). Decide: make it cancellable if that is a small, safe change on the existing registry, otherwise keep it and state the exact limit in the guide. Say which and why.

### G. P3
8. A second observability-enabled server in the same process falls back to a metric recorder that makes `/metrics` answer 503 (document or fix if small). The `tx_registry.rs` visibility widenings and new `pub` items meet the rust-guard `structure` new-item rule only under a campaign: list them in the report.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-server`; add others only if you touch them); `python -I scripts/ci/check_layout.py` is clean (the orchestrator runs it after staging; `tests/admin/activity/`, `sync/durability/` and `wire/admin/` are at 7 entries: do not add entries there, create a subfolder instead); the final report lists per item 1 to 8 the mechanism with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, the items already fixed on master, and anything not done. If the work does not fit the time limit, stop at the end of a complete item group (A, B, C, D, E are independent) and list the items not reached: do not leave a half-edited item.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/cancel-tail wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_cnt_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
