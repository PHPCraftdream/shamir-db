# Brief: end-to-end test of `Security::Definer` for `lang = "shamir"` procedures

## Why

The platform supports setuid-style functions: `Security::Definer` in
`CreateFunctionOptions` makes a function run with the rights of its owner instead of
the caller's (`Security::Invoker`, the default). For wasm functions written in Rust
this is pinned by existing tests, for `lang = "shamir"` procedures it is not:
`crates/engine/shamir-db/tests/functions/gateway/shamir_lang_procedure_e2e.rs` only
uses the default (Invoker). A `shamir` procedure reaches the database through the
`DbGateway` host imports (`db_get` / `db_insert` / `db_query` / `db_execute`), and the
gateway must apply the effective identity of the call. That path needs a test.

## What

Add an end-to-end test (or a small group) in
`crates/engine/shamir-db/tests/functions/gateway/` (a new file, wired in that
folder's `mod.rs`; keep each file under 1000 lines and the folder at most 7 entries,
split into a subfolder if needed) that, against a real in-memory `ShamirDb`:

1. creates a table and a restricted user that has NO rights on it (look at how
   `stored_proc_e2e.rs`, `facade/create_function_gating.rs`,
   `lifecycle/secrets_meta.rs` and `getter_only_e2e.rs` create users, grants and
   privileged owners, and reuse the same helpers and idioms);
2. creates a `shamir` procedure with `Security::Definer`, owned by a privileged owner,
   that reads and inserts through the intrinsics (`db_insert`, `db_get`, and the typed
   builder `batch_read` + `db_execute`), and calls it AS the restricted user:
   it succeeds and the data is there;
3. creates the same procedure with `Security::Invoker` (the default) and calls it as
   the restricted user: the call fails with the access-denied error of the platform
   (assert the error kind/message the existing Rust-wasm tests assert, not a loose
   `is_err()`);
4. optional, only if the existing tests have the same shape: a restricted user cannot
   switch the identity (create a Definer function they do not own the rights for).

Do not change production code. If the test shows that Definer does not apply to the
`shamir` path, do NOT fix it here: keep the test honest (it must fail), and report
the exact observed behaviour in the final message.

## Observable acceptance

1. The tests exist, derive their expectations from the platform rules (comments name
   the rule), no loose assertions.
2. `cargo fmt -p shamir-db -- --check`, `cargo check -p shamir-db --all-targets`,
   `cargo clippy -p shamir-db --all-targets -- -D warnings` clean.
3. `python -I scripts/ci/check_layout.py` passes.
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local
   `use`, no new `#[allow]`, no visibility widening.

## Non-goals

No production change, no new language feature, no version change.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and
`python -I scripts/ci/check_layout.py`, plus read-only git. Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --`. No `cargo build/test/run/bench/doc`, no
nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
