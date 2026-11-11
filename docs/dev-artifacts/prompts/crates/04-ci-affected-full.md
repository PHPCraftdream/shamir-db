# CI-AFFECTED-FULL: the full CI tier tests only what a change can affect

Owner's rule: do not test all crates on every change; read the real test matrix by climbing the dependency tree
from the edited crates up to all their consumers (reverse dependency closure, normal + dev + build edges). The
topmost crate must not trigger the tests of other crates. Today only the draft "fast" tier does this
(`scripts/ci/matrix/affected_crates.py`); the FULL tier (the merge gate) always runs the whole workspace. Measured on the
last full run: the Windows `integration` job (32 min) and the Windows archive build (9.5 min) are the critical path;
a mean commit affects 7.3 of 32 crates and ~756 of 1059 test-seconds. Target: -29% test time and the same share of
build time per PR, with the safety nets below.

## Design (fixed; implement exactly this, do not redesign)

1. **New job `affected`** in `.github/workflows/ci.yml` (`needs: plan`, `if: needs.plan.outputs.full == 'true'`,
   ubuntu, 5 minutes, full-history checkout like the fast tier, python only, no Rust toolchain). It runs
   `scripts/ci/matrix/affected_crates.py` and publishes outputs:
   * `args`: `--workspace`, an empty string, or `-p a -p b ...` (same contract the fast tier already validates;
     keep that validation loop and move it into the script's own output check so both tiers share it);
   * boolean flags (string `true`/`false`): `engine` (shamir-engine in the set or workspace), `ts` (any path under
     `crates/shamir-client-ts/` changed, or `shamir-query-types` in the set, or workspace), `node` (`shamir-client`
     or `shamir-client-node` in the set, or workspace), `docs_only`.
   Event rule (decided inside the script via `--event`): `pull_request` => diff against the PR base
   (`origin/master`); `push` to master, `workflow_dispatch`, `schedule` => ALWAYS `--workspace` (full matrix kept
   for master and for the weekly run with macOS); any change to `Cargo.lock`, root `Cargo.toml`, toolchain files,
   `deny.toml`, `.cargo/`, `.config/`, `scripts/`, `.github/`, or any non-`crates/`/`docs/`/`*.md` file => `--workspace`
   (already implemented, keep).
2. **No full-tier job is skipped.** Every existing job keeps running (so `merge_gate_check.py` semantics stay: a
   skipped job is still a failure), but the heavy steps inside become conditional on `needs.affected.outputs`.
   Add `affected` to every dependent job's `needs` and to the merge gate's `needs` and to `REQUIRED_JOBS` in
   `scripts/ci/gates/merge_gate_check.py` (+ its unit tests: `affected` is required in the full tier, skipped by
   design in the fast tier like the other full-tier jobs, never skipped otherwise). Steps (all `shell: bash`):
   * `clippy` (ubuntu+windows[+macOS]): `cargo clippy --locked --all-targets $ARGS -- -D warnings` where
     `--workspace` selects the whole workspace and an empty `args` skips the step.
   * `test-build`: the archive step passes the same args to `./scripts/test.sh --archive-to ... --locked $ARGS`
     (the composite action `.github/actions/build-test-archive` gets an input `packages`, default
     `--workspace`, which `cache-warm.yml` keeps default); empty args => skip build and upload, and the `test`
     job skips its download/run when args are empty.
   * `test`: runs the archive as today (it contains only the affected crates); doctests for shamir-engine only if
     `affected.engine == 'true'`.
   * `integration`: `./scripts/test.sh --full --locked $ARGS -E 'kind(test)'`; empty => skip.
   * `rustdoc`, `engine-features`, `loom-model`: run their steps only if `affected.engine == 'true'`
     (loom: also when `args` is `--workspace`); steps that always passed trivially when unaffected must still
     leave the job green.
   * `ts-unit`: only if `affected.ts == 'true'`; `node-binding`: only if `affected.node == 'true'`.
   * Unchanged: `fmt`, `source-integrity`, `rust-guard-selftest`, `test-discovery`, `cooldown`, `deny`, `plan`.
   Skipped steps must print `::notice::` naming the reason, so a green run shows what was narrowed.
3. **`scripts/ci/matrix/affected_crates.py` rewrite points** (keep its CLI and unit-test style):
   * map a changed path to its package by the NEAREST ANCESTOR DIRECTORY CONTAINING A WORKSPACE MANIFEST (from
     `cargo metadata` manifest paths), not by the assumption `crates/<dir>/`; this must keep working when the crates
     are later regrouped under `crates/<group>/<crate>/`; `vendor/hnsw_rs` is a workspace package too;
   * `crates/shamir-client-ts/` is not a Cargo package: a change there affects no Rust crate (no WORKSPACE fallback)
     but sets the `ts` flag; other unknown crate folders still fall back to `--workspace`;
   * emit the flags of (1) with `--format github-output` (key=value lines) while the default output stays the
     one-line `-p` args for the existing fast tier;
   * the reverse closure includes dev-dependencies (already) and optional dependencies;
   * a test-only crate (name `shamir-e2e`, `shamir-test-diag`) needs no special case: it is just a package.
4. **Fast tier** keeps working unchanged in behaviour; it may use the new output format helper.
5. **Docs**: `CLAUDE.md` "CI tiers" section: describe that the full tier now narrows by the dependency closure on PRs
   and runs everything on master pushes, `workflow_dispatch` and the weekly schedule (with macOS), list the global
   paths that force the whole workspace, and say how a developer forces the full workspace on a PR
   (`gh workflow run ci.yml --ref <branch>` => `workflow_dispatch` => workspace). `scripts/test.sh`: make sure
   `--archive-to`/`--from-archive`/`-p`/`-E` compose as the new steps use them (read the script; fix only what is
   needed; no behaviour change for existing invocations).

## Unit tests (required, python `unittest`, written not run: no local test runs)

`scripts/ci/tests/matrix/test_affected_crates.py`: nearest-manifest mapping (including a synthetic nested group layout
`crates/g/x`), the client-ts flag, event rule (`push`/`schedule`/`workflow_dispatch` => workspace),
the flags (`engine`, `ts`, `node`, `docs_only`), dev-dependency closure, global-path sentinel, github-output format.
`scripts/ci/tests/gates/.../test_merge_gate_check.py`: the new required job `affected` (present/missing, skipped in
the fast tier by design, unexpected skip fails).

## Constraints and checks

You cannot run tests or workflows. The only command allowed is `python -I scripts/ci/check_layout.py` (reads tracked
files only; new files are invisible to it until staged: say so). Edit `.github/workflows/ci.yml` with extreme care
(YAML indentation, quoting of `${{ }}` expressions, the `env:` block of `ci.yml` and `cache-warm.yml` must stay
identical, every job that now uses `needs.affected.outputs.*` must list `affected` in `needs`; action `uses:` pins
stay untouched). Re-read the whole diff of `ci.yml` at the end as a YAML parser would. A PR that changes `.github/`
runs the whole workspace, so this change is verified by the first normal PR afterwards; for that reason keep every
expression simple. Final message: what is done and not done, honestly, the exact outputs of the `affected` job,
and the files touched.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-ci` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: none (CI scripts and workflows only; no cargo run is needed), plus every
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
