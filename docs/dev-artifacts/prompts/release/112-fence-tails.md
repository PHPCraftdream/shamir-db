# Brief: FENCE-TAILS — the strict scan of the migration journal at open (STARTUP-3c), the remaining wording and test tail of the fence reviews, and the documented residual limits of the migration fence

Path prefixes used below: `table/` = `crates/engine/shamir-engine-table/src/`; `db/` = `crates/engine/shamir-db/src/`; `engine/` = `crates/engine/shamir-engine/src/`. Re-open every path and line before relying on it (this branch starts from `origin/master` after R4b-3 (#645) has LANDED; other executors work in parallel in other files: R4b-4 shape statistics (query runner, wire types, metrics, observability), FIX-REST step B (scheduled backup runner and metrics), the interactive-transaction lock leak (transaction registry and handlers), R5-5 (the administrative shell) and SERVER-HARDENING (the offline token CLI, secret output, Argon2 tests): do not touch their areas). This brief merges three former tasks: #1986 REL-R1c-STARTUP-3c, #1996 FENCE-FOLLOWUP and the default decision of #1990 FENCE-RESIDUALS. The full text of the fence reviews is in `<artefacts>/review_fence6.txt` and `<artefacts>/review_fence7.txt` (and `<artefacts>/review_fence.txt` for the residuals): open them for the exact findings before you act on any summary below. Nothing here is release-blocking by itself, but item A is a confirmed behaviour defect (silent loss of a journal key) and the rest are statements and tests that exceed what the code does or fail to catch a real mutant: fix them, do not defer them.

## Why

The migration journal is the record that decides whether a schema migration is applied, in doubt or absent. A corrupted physical key in the journal partition must stop the open with a typed error, not vanish silently. The documents and comments of the fence must state exactly what the code guarantees, and the tests must be able to fail against the mutants they claim to kill. The three residual gaps of the fence that need a redesign are recorded in the guide as known limitations, in one place, with the exact scope (decision of the orchestrator under the owner's standing instruction to decide toward correctness within the release; the redesign itself is a post-release item).

## What

### A. Strict scan of the migration journal at open (#1986)

`TableManager::list_stream` (`table/table_manager/accessors/table_manager_streaming.rs` around line 143) calls a decoder that silently skips malformed physical ids (`table/support/model/table.rs` around line 269), so the migration journal receives neither a key nor an error when a key in the partition is corrupted. Required: a strict scan at the lower layer (engine-table or storage) that returns a typed error for the migration journal, while ordinary reads of other partitions stay isolated (a corrupted key in an unrelated partition must not break them); the journal open path propagates the typed error and the server refuses to open with a clear, path-free message. Tests: restart over a journal partition with a corrupted key fails the open with the typed error (write the corrupted key through the storage seam the tests already have, do not hand-edit files); the ordinary partition scan behaviour is unchanged (a test that a corrupted key elsewhere is still skipped with the existing behaviour); the strict path is the one the migration journal open uses (a mutant that routes the journal through the lenient decoder must fail a test). Check the intersection with the DENY-CHRONO task (it may touch the same crate for a different reason) and say in the report what you found.

### B. Wording tail (all P3, from `review_fence6.txt` and `review_fence7.txt`)

1. The guide and the comments in `binding_scan.rs` (lines about 19-31, 61-65, 93-100) promise the common refusal too broadly: a holder with a busy migration slot gets the existing fence refusal EARLIER and it may name the database; only a generic `Conflict` carries no path; a cold table gives no flag but may give a stored binding; the inner store ignores the flag but the envelope counts for DROP. State exactly what the code does, in the code comments and in the guide (Russian, `docs/guide-docs/guide/07-operations.md`).
2. `table_manager_validators.rs` lines about 151-157 describe the cancellation AFTER a successful flush and BEFORE publication: there is no suspension point there; the cancellable case is an effect in the store while the future has not completed. Lines about 159-164 about the present-empty case concern BIND, not UNBIND (unbinding a missing binding returns before the write). `table_management.rs` lines about 303-308 describe the loss of validation as unavoidable: if the old envelope survived, rename loses the flag but may keep the binding. Correct each text to the code.

### C. Test tail (all P3, from `review_fence6.txt` and `review_fence7.txt`)

1. The `FlushFaultStore` fixture breaks its reference cycle only on success: use an RAII detach guard or a weak reference so a failing test also releases it.
2. The admitted-replace test asserts an unchanged id, but REPLACE normally keeps the same id: compare the old artifact with a distinguishable replacement.
3. The test at `schema_rollback_tests.rs` around lines 765-815 covers the wrapper check, but removing only the admitted check at `schema_management.rs` around line 636 is not caught: add the test that catches that mutant, and a test for REMOVE/ADD through the admitted entry.
4. For every test you add or rewrite: name the mutant it kills and why it fails on the parent commit (an oracle that cannot fail is the defect class of these reviews).

### D. Residual limits of the fence (default decision of #1990: document, do not redesign)

Re-open the three code sites, confirm each statement against the CURRENT code (they were written from a review of an older head), and write ONE subsection of known limitations in the guide (Russian) and the matching paragraph of the design text of the migration fence (find it with grep under `docs/dev-artifacts/design/`), each statement tied to what the code does today:
1. Ordinary DDL admitted BEFORE the fence (`admin_table_index.rs` around lines 342-378) can create an object with a different definition between the `pre_state=absent` record of an Apply and its effect; resume then accepts it by name (`apply_operations.rs` around 695-719, `ddl_postconditions.rs` around 120-144). Pre-existing admission-only limit; a fix needs a re-check of the fence at the effect or a database reservation held across DDL execution.
2. `RENAME DATABASE` publishes the new catalogue row and then deletes the old one in a separate step: a crash between them leaves two names with one identity and a reopen rejects it (`db_management.rs` around 331-367, `identity.rs` around 82-181). Pre-existing.
3. The replication pull apply writes the destination through an unfenced path (`admin_replication.rs` around 677-722).
Also say in the same subsection what the operator does in each case (the existing recovery steps in the runbook, if any: link, do not duplicate). Documented residual double faults (a binding orphan after a restart, a `bound_in` divergence after an unbind migration) remain limits and are listed there too. Do not change production behaviour for item D; if confirming a statement shows the code already changed, write what is true.

## Do not

Redesign the fence, add a reservation, re-check the fence at an effect, change the replication apply path or the database rename; change a wire tag or a stable error code; add a dependency, a config key or a metric; widen the visibility of an existing item; weaken or delete a test; use sleeps for synchronization; add `#[allow]`.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for every touched crate (expected: `shamir-engine-table`, `shamir-db`; also `shamir-engine` and `shamir-server` only if you touched them or a downstream match needs it: say which); `python -I scripts/ci/check_layout.py` is clean (I run it after staging); the final report lists per item A, B1-B2, C1-C4, D1-D3 the mechanism or the text with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, and anything not done.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/rel-fence-tails wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_fnc_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
