# Brief: CI job `wasm32 guest check` (part A)

## Why

Guest code that runs inside WASM (the `shamir-sdk` crate and what it pulls in:
`shamir-query-builder`, `shamir-query-types`, `shamir-types`) is only ever compiled for
the host today, so a change that breaks the `wasm32-unknown-unknown` build is found by
nobody. A known suspect: `getrandom 0.3` in the chain query-builder -> types -> rand
does not build for `wasm32-unknown-unknown` without a feature/cfg.

## What

1. In `.github/workflows/ci.yml` add a job `wasm32 guest check` (ubuntu-latest):
   * toolchain step with the target `wasm32-unknown-unknown` (follow how the other jobs
     install the toolchain and cache; the workflow-level env blocks must stay
     identical, see CLAUDE.md "CI tiers");
   * `cargo check -p shamir-sdk --target wasm32-unknown-unknown --locked` and
     `cargo check -p shamir-sdk --features query-builder --target wasm32-unknown-unknown --locked`
     (read `crates/wasm/shamir-sdk/Cargo.toml` for the exact feature names);
   * `cargo clippy -p shamir-sdk [--features ...] --target wasm32-unknown-unknown --locked -- -D warnings`.
2. Tiering: full tier job, listed in `needs:` of the `merge-gate` job and handled by
   `scripts/ci/gates/merge_gate_check.py` like the other gating jobs (read that script
   and its unit tests, extend both). In the fast tier it runs only when the affected
   set contains `shamir-sdk`, `shamir-query-builder`, `shamir-query-types` or
   `shamir-types` (use the outputs of `scripts/ci/matrix/affected_crates.py` the same way the
   other gated jobs do; add unit tests there only if you change the script).
3. Because there is no way to run CI here, read the existing jobs very carefully and
   copy their idioms exactly (job ids, `if:` conditions on the `plan`/`affected`
   outputs, cache keys, `timeout-minutes`).
4. If the dependency chain cannot build for wasm32, do NOT paper over it in the
   workflow: find the cause from the crate manifests (the exact crate, version and
   missing feature) and state it in the final message with the smallest proposed
   manifest fix; apply the fix only if it is a feature/cfg line in a manifest of this
   workspace (not a version bump), with a comment naming why.

## Observable acceptance

1. The workflow validates as YAML and every `needs:`/`if:` reference exists
   (`python -I -c "import yaml"` is not available offline: re-read the file, check
   indentation and job ids by eye, and run `python -I scripts/ci/check_layout.py`).
2. The merge-gate script and its tests know the new job (skipped-by-design rules are
   the same as for other full-tier jobs).
3. No `cargo build/test`; the manifest change, if any, passes
   `cargo check -p shamir-sdk` for the HOST target is NOT run either (CI does it).
4. `cargo metadata --locked --format-version 1 --no-deps` still succeeds if a manifest
   was touched.

## Non-goals

Part B (running the tests that are skipped without a Rust->wasm toolchain,
`SHAMIR_REQUIRE_WASM_TOOLCHAIN=1`) is a later task. No version change.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo metadata --locked` and `python -I scripts/ci/check_layout.py`, plus read-only
git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --`. No `cargo
build/test/run/bench/doc`, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
