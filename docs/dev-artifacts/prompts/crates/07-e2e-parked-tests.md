# E2E-PARKED-TESTS: return 25 parked client tests to the CI

Background: the client tests that boot a real server moved into the test-only crate `crates/devtools/shamir-e2e`
(`tests/internals/`). Four files could not be wired in because they use crate-private items of `shamir-client`
(`Client::for_test`, `client::LifecycleHooks`, `server_query_version_for_test` / `set_server_query_version_for_test`,
`should_advertise_interner_epochs`, the `pub(crate)` `Client::roundtrip`, the `request_scratch_*_for_test` accessors).
They are listed in `.rust-guard/exceptions.toml` and compiled by no target, so 25 tests do not run anywhere:
`ambient_sync_tests.rs` (8), `interner_cache_tests.rs` (11), `cursor_stream_tests.rs` (3), `resume_and_scratch_tests.rs` (3).
Seven now-dead `pub(crate)` `*_for_test` seams were deleted from `shamir-client` when the dev-dependencies on the server were
removed (`git log -p -S 'for_test' -- crates/client/shamir-client/src/client/api.rs
crates/client/shamir-client/src/client/session/client.rs` or `git show` of the CRATE-E2E merge shows them: six in `client/api.rs`,
`server_query_version_for_test` in `client/session/client.rs`). Goal: every one of the 25 tests runs again, without giving
`shamir-client` a dev-dependency on `shamir-server`, `shamir-db` or `shamir-engine`, and without widening any visibility
(rust-guard rejects new `pub`).

## Method (decide per test, in this order)

Read each parked file and classify every test by what it really needs:

* **A. Fake transport only** (uses `Client::for_test` with duplex pipes and an emulated server, no `ServerLauncher`, no
  `shamir_server`): these are plain unit tests of `shamir-client`. Move them back into the client crate as in-crate tests
  under `crates/client/shamir-client/src/tests/<topic>/` (one file per topic, `tests/mod.rs` manifests, wired through the
  owning parent), restore exactly the `pub(crate)` seams they need from git history (same signatures, `#[cfg(test)]` where
  the original had it), keep the test bodies byte-identical except for paths.
* **B. Real server, public API only**: port into `crates/devtools/shamir-e2e/tests/internals/` (add to `main.rs`, or a new
  topic target if the folder is full; the folder has 7 entries today, count before adding) using only `shamir_client`
  public items and the server launcher the existing e2e files already use. Keep the assertions.
* **C. Real server AND a private item**: express the same observable assertion through public means. Example for the
  cursor-close proof (`cursor_stream_tests.rs`): after `close()` the registry entry must be gone; assert it by sending a
  raw `FetchNext` for the same cursor id from a second raw connection built like the test already builds frames
  (`shamir_transport_tcp::framing`, `shamir_connect::common::envelope`, `shamir_query_types::wire`), which is what an external
  client does; no private `roundtrip` needed. If a test truly cannot be expressed publicly, keep it out, and say exactly
  which test and why in the final message (do not leave a silent gap).

The test count must not drop: at the end list old test name -> new location for all 25.

## Cleanups

* You cannot delete files. When a parked file is fully re-homed, leave it untouched; the orchestrator removes the four files
  and their `.rust-guard/exceptions.toml` entries. Do not edit the exceptions file; say in the final message which files are
  now fully superseded.
* `crates/devtools/shamir-e2e/tests/internals/main.rs` doc comment: update it to the new state.

## Constraints and checks

Binding repo rules: at most 7 entries per folder (count by hand, the layout script cannot see untracked files), at most 1000
lines per file, one primary export per file, no local `use`, no inline test modules, no new `#[allow]`, no visibility
widening, tests in `tests/<topic>/` with manifests, JSON literals multi-line. `Cargo.toml` of `shamir-client` must keep no
`shamir-server`/`shamir-db`/`shamir-engine`/`shamir-test-diag` dev-dependency; add to the e2e crate only what a ported test
imports.

Done when: `cargo check --all-targets`, `clippy -D warnings`, `fmt --check` pass for `shamir-client` and `shamir-e2e`,
layout check passes after staging, and the final message lists the 25 tests with their new homes (or the justified
exceptions).

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-parked` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: shamir-client, shamir-e2e, plus every
dependent whose public signature or path you changed (grep the workspace). `Cargo.lock` may change only by what
this task needs: run `cargo check` once WITHOUT --locked so cargo writes it, and say so. If you believe you need a
build or test run, say so in the final message. No other shell commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
mutates the working tree or index. Only edit files; the orchestrator commits (and has already done every file
move: you cannot move or delete files, so work with the layout you find).

## Repository rules that bind this work

At most 7 direct entries per folder, at most 1000 lines per source file; `mod.rs` re-exports only; one file = one
primary export; tests live in `tests/<topic>/`, never inline `#[cfg(test)] mod`; no local `use`; no visibility
widening and no new `#[allow]`; no `impl` blocks outside the owning module (rust-guard); JSON literals in tests
multi-line; comments terse; no machine-local paths; do not mention how or where CI is run. Count folder entries by
hand for every folder you touch (the layout script cannot see untracked files) and nest instead of passing 7.
Children, if you use any, only edit files, never run commands, and get the NO-BUILD policy above quoted verbatim.
