# Brief: ENGINE-PATH-TAILS — the residuals of the read paths (a cell created before the table is known to be configured, existence disclosure through different refusals, missing audit and slow-query records for Status and GetRecord) and the loss of stable error codes on the abort paths of batches and stored functions

Path prefixes used below: `engine/` = `crates/engine/shamir-engine/src/`; `et/` = `crates/engine/shamir-engine-table/src/`; `server/` = `crates/server/shamir-server/src/`. Re-open every path and line before relying on it (this branch starts from `origin/master` after R4b-4 (#649), R5-5 (#648) and the interactive-transaction lock work (#647) have LANDED; other executors work in parallel in other files: the shell tails (`crates/server/shamir-server/shell/` and `et/support/storage/catalog.rs`: the table registration accessor is THEIRS, do not touch it), the backup tails in `shamir-sync` and `shamir-server` (scheduler, registry, retention, shutdown), SERVER-HARDENING, FENCE-TAILS (`et/table_manager/`, `et/support/model/`): do not touch their areas; if you must touch a file outside the engine and the query path, say so and keep it minimal).

## Why

The reviews of R5-3 (read paths) and of the Argon2 gate (abort paths) left CONFIRMED pre-existing defects that were deliberately not fixed at landing. Each item below is a confirmed finding: fix it, do not defer it, with the one decision recorded below.

## What (fix in this order; for each item write the failing test first where a test is possible)

### A. Read paths (from the R5-3 review)
1. `engine/` `catalog_access.rs` (~166-200) and `et/support/storage/catalog.rs` (~79-97, 131-137) create the cell of a table BEFORE checking that the table is configured: repeated requests with DIFFERENT NON-EXISTENT names grow the map (a defect shared by batch and cursors; `GetRecord` bypasses the check of the configuration in R5-3). Check the configuration FIRST and create the cell only for a configured table (note: another executor owns the table registration accessor in `catalog.rs`: do NOT edit that accessor; fix the callers in `catalog_access.rs` and the read paths so that no cell is created for an unconfigured name; if the fix needs the accessor, say exactly what and stop: the orchestrator coordinates). Test: N requests with distinct unknown names leave the map size unchanged (use the O(1) mirror counters the code has, never `scc::len()` on a hot path; a test-only count is fine).
2. DECISION (binding, mine): the answer for an ABSENT table and for an existing CLOSED table (no access) must be the SAME in ALL read paths (batch, cursor, `GetRecord`): today an absent table passes the ACL (open by default) and answers `unknown_table`, while an existing closed one answers `access_denied`, which discloses existence. Unify them: the refusal for a caller without access to a table must not depend on whether the table exists. Do it unless it breaks the R5-3 parity statement of the guide (read the guide and the parity tests first): then document the exact residual instead and say why in the report. Check every consumer of the two codes (clients, the shell, tests, the TypeScript client) and update the guide.
3. `Status` and `GetRecord` produce no audit event and no slow-query record: add them the way the sibling read paths do (same helper, same fields, no payloads or literals), with tests.

### B. Abort paths (from the Argon2 gate review: pre-existing engine behaviours)
4. Transactional batch: a failed op (for example `server_busy` from `CreateUser`) becomes `Ok(.., transaction.status = 'aborted')` with the error debug-formatted into the reason and NO stable code (`engine/query/batch/exec/batch_execute/transactional.rs` ~237-301, `runner/query_runner/helpers.rs` ~155-160); the abort metadata of a transactional `ForEach` body is DISCARDED by `execute_planned_batch_impl` (`executor.rs` ~414-452, `arms_read.rs` ~450-495). Carry the stable error code of the failing op in the aborted result (additive field of the existing result shape: no change of an existing wire tag; check the wire types, the builder, the clients and the guide), and keep the abort metadata of the `ForEach` body.
5. Stored-function path: the WASM host stringifies the gateway error and the outer `Call` builds a `QueryError` with code `None` (`db_gateway.rs` ~150-172 and ~264-282, `wasm-host` `bindings/db.rs` ~194-198, `function_invoker.rs` ~45-53): preserve the stable code across the host boundary where the host ABI can carry it without a new import (if it cannot, document the limit exactly and say why).
6. Native `Execute` replaces an encoded busy reply with `result_too_large` when the client's `max_result_size` is tiny (`server/engine/db_handler/repo_admin/execute.rs` ~218-221, ~318-365): a busy refusal must survive a tiny result limit (the reply is small and fixed): fix and test.
7. Guide: the Argon2 round-2 guide states these three limits once as known limitations: update it to what is true after your changes (and keep the statement of every limit you could not remove).

## Do not

Change a wire tag or an existing stable error code; add a dependency, a config key or a metric; widen the visibility of an EXISTING item (run `python -I <orch>/vis_widen.py <worktree>` after your last edit and report its output; `<orch>` is the folder of the cargo wrapper `cg.sh` named in the rules below; the CI gate `structure` rule `VIS_WIDENING` has no exemption for tests); respect the layer rule of `shamir-engine` (`repo < table < validator < tx < query < instance < db_instance`; `.rust-guard/layers.toml`; `.rust-guard/layers_baseline.toml` must not change); weaken or delete a test; use sleeps for synchronization; add `#[allow]`; touch the table registration accessor (`et/support/storage/catalog.rs` `table_registration`).

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-engine`, `shamir-engine-table`, `shamir-query-types`, `shamir-query-builder`, `shamir-wasm-host`, `shamir-db`, `shamir-server`, `shamir-client`, `shamir-e2e` if a test enumerates what you changed); `python -I scripts/ci/check_layout.py` is clean (I run it after staging); the final report lists per item 1 to 7 the mechanism with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, and anything not done. If the work does not fit the time limit, stop at the end of a complete item (A and B are independent) and list the items not reached: do not leave a half-edited item.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/rel-engine-path-tails wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_ept_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
