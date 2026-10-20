# CI-03b — compiled gates: rustdoc, engine feature matrix, test-discovery diff, aggregate merge gate

## Why

CI-03a (`tools/rust-guard`, merged) is the *static* half of the safety net for
the `shamir-engine` layout sweep. It cannot prove that CI actually compiles
every configuration, that docs still build, or that the set of tests the
compiler discovers is unchanged. This brief adds the *compiled* half. Spec
source: the adjudicator report sections "2(b) compiled discovery inventory",
"2(c) Rustdoc gate", "2(d) Feature matrix", "Baseline integrity and
required-check integrity".

Builds and tests run only in GitHub CI. Locally the executor may run only the
allowed gates from the policy (`cargo check`/`clippy`/`fmt` scoped,
`python -I scripts/ci/check_layout.py`). `cargo doc`, `cargo nextest`,
`cargo test` are NOT allowed locally: the new CI jobs are verified by pushing.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`
  or any git command that mutates the working tree or index. Only edit files;
  the orchestrator commits.
- Compile / build / test policy: the verbatim text is in
  `.scratch/POLICY_VERBATIM.md`; it binds you and every child.
- Never use the shell builtin `xargs`. Capture exit codes without pipes
  (`cmd > log 2>&1; echo exit=$?`). Logs under `.scratch/`.
- Layout limits apply to everything you add: <= 7 entries per folder,
  <= 1000 lines per file, imports at the top, `mod.rs` re-exports only, no
  `#[allow]`. `scripts/ci/` already holds 5 entries plus `tests/`: group new
  scripts into a subfolder if the count would exceed 7.
- Pin every third-party action by commit SHA exactly like the existing jobs
  (copy the `uses:` lines from `ci.yml`); no new action unless unavoidable.
- No private machine paths, no version bumps.

## Deliverables

### 1. Rustdoc gate (`ci.yml`)

- Job `rustdoc` (full tier: same `if:` as `clippy`) on ubuntu-latest:
  `cargo doc -p shamir-engine --no-deps --document-private-items --locked`
  with `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -D rustdoc::private_intra_doc_links"`,
  once with default features and once with `--no-default-features`.
- Fast tier (`fast` job): the same two commands as a step that runs only when
  the affected-crates output contains `shamir-engine` or `--workspace`.
- Existing broken links the new gate reveals are fixed in place in the doc
  comments (minimal edits; the first CI run lists them; you cannot run
  `cargo doc` here, so scan the engine for `[`path`]` links that cannot
  resolve only if cheap — otherwise leave to the next round with CI logs).
- Downstream crates (shamir-db, shamir-server) are NOT documented in this
  stage; record that as a known limitation in the README/CLAUDE.md text.

### 2. Engine feature matrix (`ci.yml` + assertion script)

- Job `engine-features`, matrix of isolated `-p shamir-engine` cells, each
  `cargo check -p shamir-engine --all-targets --locked <flags>` (isolated
  package selection with resolver 2; never a workspace invocation that can
  unify defaults back in):
  1. `--no-default-features`
  2. `--no-default-features --features fjall`
  3. `--no-default-features --features test-util`
  4. `--no-default-features --features fjall,test-util`
  5. default features `--features test-util,fts_indexed_long`
  6. `--features loom` (loom cell)
- Full tier: all six cells on ubuntu, windows, macos (`fail-fast: false`).
  Fast tier: cells 1 and 5 on ubuntu, only when `shamir-engine` is affected.
- Feature-resolution assertion: a script (python, stdlib only, with unit
  tests under `scripts/ci/tests/` or the subfolder you create) that, for a
  cell, computes the enabled feature set of `shamir-engine` from cargo
  (`cargo tree -p shamir-engine -e features ...` or `cargo metadata`; choose
  the one that really reflects `-p` + flags) and fails when it differs from
  the expected set: cell 1 = {}, 2 = {fjall}, 3 = {test-util},
  4 = {fjall, test-util}, 5 = {default, all-backends, fjall, test-util,
  fts_indexed_long}, 6 = {default, all-backends, fjall, loom}. "No defaults"
  must be proven, not just a label.
- Loom discovery: the existing `loom-model` job must additionally fail when
  `cargo nextest list -p shamir-engine --features loom --lib -E 'test(loom_model)'`
  finds zero tests (also pass `--no-tests=fail` to its `nextest run` if the
  pinned nextest 0.9.137 supports it).
- Run every cell's `cargo check --all-targets` and `cargo clippy
  --all-targets -- -D warnings` LOCALLY now (one cargo process at a time,
  `CARGO_BUILD_JOBS=4`); reduced-feature cells may expose missing `cfg`
  guards in engine code — fix them in place with the narrowest edit. Report
  per-cell exit codes.

### 3. Test-discovery diff (rust-guard subcommand + CI job)

- New `rust-guard discovery` subcommand in `tools/rust-guard`:
  `rust-guard discovery --base-list base.json --head-list head.json --delta <delta.toml>`
  where the lists are `cargo nextest list --message-format json` outputs
  taken under identical toolchain, OS, features, profile and target
  selection. Compare test identities (binary id + test name) through the
  delta's `[[moved]]`/`[[renamed]]` mappings (reuse the existing delta parser
  and types). Findings: test lost, unexpectedly duplicated, newly ignored
  (ignored status changed), target/binary membership lost, empty list
  (a list with zero tests is a failure, not a pass), unused allowance.
  Doctests and loom are listed separately (`--kind lib|loom|doc`).
  Missing/failed base list => explicit "blocked comparison" finding, never an
  empty baseline.
- CI job `test-discovery`: runs only when `.rust-guard/stages.toml` has an
  active campaign whose packages the diff touches (reuse
  `scripts/ci/rust_guard_select.py`; with `campaigns = []` it is a no-op that
  prints a notice and passes). It builds the merge-base list in a separate
  `git worktree` of the base commit under a different `CARGO_TARGET_DIR`,
  caches the base list with `actions/cache` keyed by base SHA + OS + config
  (never across configurations), builds the head list, runs the subcommand.
  ubuntu in the fast tier; ubuntu + windows + macos in the full tier, default
  configuration plus the `test-util,fts_indexed_long` configuration.
- Self-test fixtures (CI job `rust-guard self-test`, already exists): base/
  head list pairs for lost test, duplicate, newly ignored, empty list, valid
  move, valid rename, unused allowance, missing base. Keep folder limits.

### 4. Aggregate merge gate (`ci.yml` + script)

- Job `merge-gate` (display name `merge gate (required)`), `if: always()`,
  `needs:` every gating job (fast, fmt, clippy, test, integration, ts-unit,
  cooldown, loom-model, source-integrity, rust-guard-selftest, rustdoc,
  engine-features, test-discovery). It passes `toJSON(needs)` plus the event
  name and the draft flag to a stdlib python script with unit tests.
- Semantics: a needed job whose result is `failure` or `cancelled` fails the
  gate; `skipped` fails the gate unless that job is skipped BY DESIGN for this
  tier (draft PR: full-tier jobs are skipped by design and `fast` must be
  `success`; ready PR / master push / workflow_dispatch: `fast` is skipped by
  design and every full-tier job must be `success`). On a DRAFT PR the gate
  itself still FAILS with the message "draft PR: full matrix not run, not
  merge-ready" — draft fast-tier success is not authorisation to merge. The
  script's unit tests cover every transition above, including an
  unexpectedly skipped required job and an unknown job name.
- Document in CLAUDE.md ("Code quality" subsection next to the rust-guard
  text) and CONTRIBUTING if it describes CI: what each new job gates, that the
  merge gate is the one required check, and how to read a draft PR's gate.

## Not in scope

Any engine behaviour change (only the minimal cfg/doc-link fixes that make the
new cells and the rustdoc gate pass), changes to `check_layout.py` rules,
benches, downstream-crate rustdoc, branch-protection settings (the owner sets
the required check name after this lands).

## Final message

Per deliverable: files and line counts (all <= 1000), the YAML job names and
their `if:`/`needs:`, per-cell local exit codes for `cargo check`/`clippy`,
the fixtures and which finding each triggers, what fails closed, every
limitation knowingly left, and the child-verifier verdict line copied
exactly.
