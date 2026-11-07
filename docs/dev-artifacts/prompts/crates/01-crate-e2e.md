# CRATE-E2E: a test-only crate for the client tests that need a real server

Goal (user's rule): the topmost crate must not trigger the tests of other crates. `shamir-client` has
`dev-dependencies` on `shamir-server`, `shamir-db` and `shamir-engine`, the only upward edges of the workspace
graph, so a change in server/db/engine reruns the client (and client-node) tests. This task removes them.

## Already done by the orchestrator (do not redo, you cannot move files)

* `git mv` of every client test that boots a real server into the new crate directory `crates/shamir-e2e/`:
  `tests/{batch,cursor,matrix}/`, `tests/smoke.rs`, `tests/smoke_local.rs` (unchanged contents, they use only the
  public client API) and six in-crate tests that were in `crates/shamir-client/src/tests/` and are now
  integration tests in `crates/shamir-e2e/tests/internals/` (listed in `tests/internals/main.rs`):
  `ambient_sync_tests`, `auth_identity_tests`, `cursor_stream_tests`, `interner_cache_tests`,
  `resume_and_scratch_tests`, `v2_passthrough_tests`.
* The matching `mod` lines were removed from `crates/shamir-client/src/tests/{auth,client,interner,wire}/mod.rs`.
* `crates/shamir-e2e/src/lib.rs` exists (a doc comment only). `crates/shamir-e2e/Cargo.toml` does NOT exist yet.

## Deliverables

1. `crates/shamir-e2e/Cargo.toml`: package `shamir-e2e`, `publish = false`, same edition/license/lints as
   `shamir-client`, `[lib] doctest = false`; dependencies only what the moved tests import (derive it from
   their `use` statements: `shamir-client`, `shamir-server`, `shamir-db`, `shamir-engine`, `shamir-types`,
   `shamir-query-types`, `shamir-query-builder`, `shamir-connect`, `shamir-transport-*`, `shamir-test-diag` with
   the `tracing` feature if the tests use it, `tokio`, `tempfile`, `bench-scale-tool` if used, and so on) as
   `[dev-dependencies]`; test targets are auto-discovered (`tests/<dir>/main.rs`); copy any `[[test]]` section
   the old client manifest had for these tests. Register nothing else in the workspace: the root manifest uses a
   glob so the crate is picked up (verify by reading the root `Cargo.toml`).
2. The six `tests/internals/*` files used crate-private client items (`crate::client::LifecycleHooks`,
   `crate::client::should_advertise_interner_epochs`, `crate::builder::cursor::fetch_next`, and
   `crate::ClientError`/`crate::error::ClientError`). Rewrite them to the PUBLIC `shamir_client` API only
   (`use shamir_client::...`; `shamir_client::builder` is already a public re-export). You may NOT make any item
   more visible and may NOT add a `pub` re-export of an internal. Replace a private call with the public
   equivalent only when the behaviour under test is identical. If a test truly needs a private item, do not hack it
   and do not delete or empty it: leave the file as it is, keep it out of `tests/internals/main.rs` so the crate
   compiles, and report the test name and the exact private item in the final message (the orchestrator decides).
3. `crates/shamir-client/Cargo.toml`: remove `shamir-server`, `shamir-db`, `shamir-engine` from
   `[dev-dependencies]` (and anything that becomes unused). If a remaining client test still needs one of them, find
   it with grep and report it; do not keep the dev-dependency.
4. Fix every reference to the moved test paths: `scripts/test.sh` scopes, `.config/nextest.toml` overrides
   (filters like `package(shamir-client)` for slow tests), `.github/workflows/*`, `scripts/ci/*`,
   `.rust-guard/*.toml`, `scripts/ci/layout_baseline.txt` (entries for moved paths: lower or remove, never add),
   and docs that name the old paths. grep for `shamir-client` in those places and for the moved file names.
5. `CLAUDE.md`: the crate count and the list of crates (add `shamir-e2e`, test-only, one sentence).

## Success criteria

`cargo metadata`-style reading of the manifests shows no workspace crate depends upward through `shamir-client`
(client's dev-deps: only downward crates such as `shamir-test-diag`, `tokio`, ...), `shamir-e2e` depends on
client+server+db+engine, `cargo check -p shamir-client --all-targets`, `cargo check -p shamir-e2e --all-targets`,
`cargo clippy` for both with `-D warnings`, `cargo fmt -p` for both, layout script. Final message: what is done
and not done, the list of tests that could not be ported and why, files touched.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-e2e` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: shamir-client, shamir-e2e (new), plus every
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
