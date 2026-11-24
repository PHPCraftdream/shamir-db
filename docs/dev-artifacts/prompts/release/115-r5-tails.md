# Brief: R5-TAILS — the open items of the shell reviews R5-3 and R5-4 (tests that the harness could not carry, selector rules, the activity allowlist, the table registration accessor, the guide) and the weaker flag pin of R5-5

Path prefixes used below: `shell/` = `crates/server/shamir-server/shell/`; `shtests/` = `crates/server/shamir-server/tests/admin/shell/`; `et/` = `crates/engine/shamir-engine-table/src/`. Re-open every path and line before relying on it (this branch starts from `origin/master` after R5-5 (#648) and R4b-4 (#649) have LANDED; other executors work in parallel in other files: the backup tails in `shamir-sync` and `shamir-server` (scheduler, registry, retention, shutdown), SERVER-HARDENING (`shamir-connect`, `shamir-server-security` logging, the token CLI), FENCE-TAILS (`shamir-engine-table` validator binding and the schema-migration journal: `et/table_manager/` and `et/support/model/` are theirs: coordinate by staying out of `table_manager_validators.rs`, `table.rs` and the strict-scan files; the table registration accessor below lives in `et/support/storage/catalog.rs`, which they do not touch), the engine read-path tails: do not touch their areas; if you must touch a file outside the shell and `shamir-engine-table`, say so and keep it minimal).

## Why

The reviews of R5-3 (rights and activity views), R5-4 (shell activity and cancellation) and R5-5 left CONFIRMED defects, missing tests and imprecise texts that were deliberately not fixed at landing. Each item below is a confirmed finding: fix it, do not defer it.

## What (fix in this order; for each item write the failing test first where a test is possible)

### A. Table registration accessor (R5-3, P2)
1. `et/support/storage/catalog.rs` (~79-97, 131-138, 229-233): the accessor `table_registration` is a get-or-insert under TWO separate map operations: a drop between `contains_key` and `insert` re-creates an empty cell for an already-unconfigured name, and a vanished table can answer `access_denied` (changed registration) instead of `unknown_table` (both outcomes are refusals, but the code differs); a parent that vanished and re-appeared before the re-probe can read as `unknown_table`. Make the accessor a single atomic operation of the map (an entry API of the concurrent map the code already uses: `scc` entry or `insert_sync` semantics) and make the outcomes consistent; test with a handshake or a deterministic interleaving seam (no sleeps).

### B. Tests (R5-3 and R5-4), each with a named mutant
2. A parked request cancelled through the SHELL or the Rust client (today proven only at handler level in `tests/admin/activity/cancel_lock_tests.rs`; the shell harness has no `ShamirDb` access: use the real-server fixture of `shtests/`); a foreign cancel without `ActivityView:Manage` is refused; a `requests` golden with a parked row of ANOTHER session; a scan that the session key and tag appear nowhere in stdout, stderr, argv or the profile; the deadline end test through a real session (needs an injectable clock and a fake `SessionClient`: if the code has no such seam, add the narrowest `cfg(test)` seam and list it); count requests on the server, not accepted connections; the missing-tag case for `chmod`.
3. R5-3 P3 tests: the head_tail 10 ms/40 ms test can pass the mutant "check only before the scan" at a 10 ms delay (use a handshake); the replace test checks the batch but not `destroy.existed`/`recreate.created`, and the parked read's key differs from the seeded replacement row's key (fix the fixture); parity and quarantine tests have an unbounded setup `await` (bound every await under the 180 s kill); the golden does not bound `client.close().await`; the unsigned-identifier projection test builds the `Big` itself and skips the chain `RepoIdentity -> to_qv` (drive the real chain); there is NO test for a LATE quarantine (after the parking).
4. R5-3 P3 values: B4 accepts the text "7" for a number (the engine emits such values as integers) and a `Big` larger than `u64::MAX` in-process: decide with the encoder (accept only what the engine can emit) and test both.
5. R5-5 tail: the source half of the flag pin in `shtests/maintenance/explain.rs` (`explain.rs` ~189-203 and ~437-444) asserts `src/main.rs.contains(declaration)`; several needles are not unique to the subcommand they are attributed to (`db: Option<String>`, `table: Option<String>` also live in `AccessTree`; `apply`, `pretty`, `json` in both `AccessTree` and `Doctor`), so moving `--to` from `Backup { to }` to `Restore { .. }` leaves the test green. Pin the declaration TOGETHER with its enclosing variant (parse the block of the variant from the source, or use the real `clap` parsers through a wrapper in-crate where the subcommand is declared in the binary): the move of a flag between inline subcommands must fail the test.

### C. Code
6. `parse_selector` rules for `table` and `function`: the 255-byte limit of the PHYSICAL name (`et/` `names.rs`) is not applied; add it (with the same refusal the engine gives) and test; a test for a non-bootstrap holder of `Manage(Root)`; `rights show root` must not send a request; user names in PRECIS form (not only ASCII up to 64 bytes) must pass the activity field allowlist: align the allowlist with the identity rules of the server (read how the server validates user names) and test with a non-ASCII name.
7. Split `shell/commands/rights/access.rs` (859 lines) along its seams: a new file for a coherent part (one primary item per file; the file is under the limit but the review asks for the split): moves keep the module path where other code names it (use `#[path]` like the R5-5 regrouping did, or re-exports), and NO visibility of an existing item may grow (the CI gate `structure` rule `VIS_WIDENING` has no exemption for tests or moves of items to another module path: a moved item is a new item, but a changed visibility keyword on it is a widening of the item compared with the base: keep every keyword as it was).

### D. Documents
8. Guide (`docs/guide-docs/guide/07-operations.md`, Russian): re-check the rights and activity sections against the final code; the gaps of the fields `status`, `version`, `subscriptions` (their contracts); stale comments (the name limit points to a wrong validator path).

## Do not

Change a wire tag, a stable error code or the documented behaviour of the shell beyond the items above; add a dependency, a config key or a metric; widen the visibility of an EXISTING item (run `python -I <orch>/vis_widen.py <worktree>` after your last edit and report its output; `<orch>` is the folder of the cargo wrapper `cg.sh` named in the rules below); weaken or delete a test; use sleeps for synchronization; add `#[allow]`; touch the validator binding files or the journal of FENCE-TAILS.

## Observable acceptance

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` exit 0 for `shamir-server`, `shamir-engine-table` (and any other touched crate); `python -I scripts/ci/check_layout.py` is clean (I run it after staging); the final report lists per item 1 to 8 the mechanism with `path:line`, the test and the mutant it kills (and why it fails on the parent commit), what the orchestrator must observe in CI, every widening and every new cross-module item, the grep result of every cited test name, and anything not done. If the work does not fit the time limit, stop at the end of a complete item and list the items not reached: do not leave a half-edited item.

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

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: the wrapper queues you; never poll), ALWAYS as `bash <orch>/cg.sh env RUSTC_WRAPPER= CARGO_TARGET_DIR=<target-exec>/rel-r5-tails wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <artefacts>/exec_r5t_<name>.log 2>&1` (use exactly this target directory, never `cargo clean`, never a local `target/` folder, never `-j 2`, never a pipe through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper; every log stays OUTSIDE the repository under <artefacts>/). Only you, the top executor, run cargo; workers edit files and read code only and must be told so. The target folder is COLD and several executors share one cargo lane: the first `check` can take 25 minutes or more behind the queue, so write the change and the tests first, queue ONE combined `cargo check -p ... --all-targets` (all touched crates in one command), and use the wait for the guide text; never queue per-crate checks that one combined check covers.
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
