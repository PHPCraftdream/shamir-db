# Brief: switch CI to the test matrix (CI-MATRIX-SWITCH)


## PARALLEL MODE — read first

Stage 1 (`15-test-matrix-cli.md`) is being built IN PARALLEL by another executor in another worktree; its code is
NOT in this tree (`scripts/ci/matrix/test_matrix.py`, `edges.toml`, the shadow step are absent, and stage 1 will move
`affected_crates.py` and its test into `scripts/ci/matrix/`). Therefore, in this stage:

* Code against the CONTRACT written in `docs/dev-artifacts/prompts/ci/15-test-matrix-cli.md` (section 1: the JSON
  output, `--format github-output`, flags, `args`; sections 2-5: semantics). Do not implement the command. Test your
  consumers with hand-built JSON fixtures that follow that contract.
* Do NOT move, delete or edit `scripts/ci/affected_crates.py` or its test; stop USING it in workflows instead (the
  orchestrator deletes it while merging with stage 1). Do not add a shadow step.
* New Python modules go to `scripts/ci/matrix/` (at most 2 new files: `tree_green.py`, `filterset.py`) and their tests
  to `scripts/ci/tests/matrix/` (at most 2 new files); both folders will be shared with stage 1, so keep the file
  names exactly these and keep each folder you create at <=3 entries. `scripts/ci/gates/merge_gate_check.py` and its
  tests are yours alone.
* `ci.yml` is edited by both stages; the orchestrator merges by hand. Keep your edits to `ci.yml` surgical and
  mark each edited job in the final report so the merge is mechanical.
* The "shadow comparison must show a subset" safety-net item cannot be executed in this tree: instead implement the
  `matrix self-check` job and state in the PR description that the comparison is performed by the orchestrator after
  the merge with stage 1.

## Why

Stage 1 (`15-test-matrix-cli.md`, landed) added `scripts/ci/matrix/test_matrix.py` and ran it in shadow mode: CI
printed the new selection next to the old one but kept running the old one. Read that brief, the command's module
docs, `scripts/ci/matrix/edges.toml`, the shadow summaries of recent PR runs and the `--backtest` report first.
Owner requirement (unchanged): a crate's tests run only when something that really affects it changed (its own code
or a dependency); a change travels up the chain to the top crates, which run only their own tests; sibling branches
do not run; no event selects the whole workspace by itself; "all crates" is only the honest answer of the computation
(toolchain, profiles, an unclassified path) or an explicit manual input.

## What

1. **Consume the matrix.** In `.github/workflows/ci.yml` (fast tier and full tier) and `.github/actions/build-test-archive`:
   * the `affected` job outputs the matrix JSON plus the flags of `test_matrix.py` (`--format github-output`);
   * lib tests: crates whose matrix row has `lib: true`; the nextest archive is built for exactly those packages
     (`-p ...`); crates with `check_only` get clippy/check only;
   * integration: for each row, `integration: "all"` -> the crate's `kind(test)` binaries, a list -> exactly those
     binaries (`-E 'package(X) and binary(Y)'` combinations; build the filterset in `scripts/test.sh` or a small
     helper next to the matrix command, with unit tests; never string-concatenate untrusted names: names come from
     `cargo metadata` and must match `^[A-Za-z0-9_-]+$`);
   * clippy, wasm32, rustdoc, engine feature cells, `node-binding`, `ts-unit`, `loom`, `deny`: gated by the matrix
     flags/rows exactly as today's gates are gated by the old outputs (map each old output to its matrix
     equivalent in a table in the PR description);
   * the `canary` mechanism for pipeline changes (`ci.yml`, `.github/actions/**`): the matrix already selects the
     canary crates and the `pipeline` flag; make sure every job kind actually runs on its canary.
2. **Events.**
   * `pull_request`: base = merge-base with `origin/master`.
   * `push` to `master`: range `github.event.before..github.event.after`. Skip all test jobs when the tree hash of
     `after` equals the tree hash of a commit whose full run was green (a PR head that was merged by squash has the
     same tree as the merge result): implement as a small script `scripts/ci/matrix/tree_green.py` that asks
     `gh api` for successful `ci.yml` runs on `pull_request` with `head_sha` whose tree equals the pushed tree
     (bounded to the last 30 runs); when in doubt run the computed matrix (never skip on an API error). The merge
     gate must understand "skipped by tree equality" with an explicit notice.
   * `workflow_dispatch`: inputs `range` (default empty = last commit), `crates` (space-separated list; bypasses
     the computation, validated against workspace names), `scope` (`computed` default | `workspace`). `workspace`
     is the only way a manual run takes everything; it is documented as the pre-release button.
   * `schedule`: no whole-workspace run. Weekly: only the top crates (`shamir-server`, `shamir-e2e`) on
     ubuntu, windows AND macos (their lib + integration). Keep `cache-warm.yml` as it is.
   * Remove `WORKSPACE_EVENTS` and the old automatic `--workspace` paths; delete the old selector (or leave a thin
     wrapper that calls the new command, if other workflows still import it; grep both names everywhere).
3. **Merge gate.** `scripts/ci/gates/merge_gate_check.py` (and its tests, `test_gate_inventory.py`): a job skipped
   because the matrix selected nothing for it, or skipped by tree equality, passes with a recorded reason; a job
   skipped for any other reason still fails the gate. Keep the draft rule and the `[ci full]` marker semantics: the
   marker still decides fast vs full tier, it no longer selects tests.
4. **Docs.** `CLAUDE.md` "CI tiers" and the compile/test policy paragraphs: replace the old selector description and
   the "whole workspace" sentences by the matrix rules (events, canary, manual `scope=workspace`, weekly top crates).
   `docs/guide-docs` contributor notes if they mention the old selector.

## Safety net (must be in this change)

* A job `matrix self-check` (cheap, ubuntu) runs on every PR: `test_matrix.py --check` plus `--backtest` over the
  last 30 first-parent commits asserting that for each commit the selection contains every crate whose production
  code changed (the property of stage 1), so a regression of the computation is caught before it hides tests.
* The shadow comparison step is removed only after this stage's PR shows, in its own CI run, that the selected set
  equals the set the old selector would have chosen or is a documented subset (paste it in the PR description).

## Tests (run in CI, not here)

Unit tests for every new Python module and for the gate changes (events: pull_request, push with before/after,
dispatch with each input, schedule; tree-equality skip incl. API failure; invalid crate names rejected; gate accepts
matrix-skips and rejects other skips). Existing tests stay green (moved or updated with the reason, never weakened).

## Observable acceptance

`python -I scripts/ci/check_layout.py` passes; `actionlint`-clean workflow syntax as far as can be checked by reading
(no local runs of workflows); the PR's own CI run is a docs/CI-class change and must show: canary jobs ran, the
matrix self-check ran, the gate is green. Final report lists: every old output -> matrix mapping, every removed
whole-workspace path, anything unverified.

## Constraints

Compilation, builds and tests run on GitHub CI, not on this machine. Banned unless the user explicitly asks:
`cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`,
`npm`/`tsc`/`vitest` builds and test runs, WASM builds, e2e harnesses, any release or full-workspace build, and the
Python unit-test suite (CI runs it). Allowed locally, only these: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` (not needed here),
`python -I scripts/ci/check_layout.py`, `cargo metadata --locked --no-deps --format-version 1`, running
`test_matrix.py` itself, read-only git and read-only `gh run view` of existing runs. If you believe you need a build
or test run, say so in your final message.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Threat model: the workflow now takes decisions from computed JSON and from a pushed range. Inputs an attacker
controls: branch names and PR titles (never interpolate them into shell), crate names from the `workflow_dispatch`
input (validate against the workspace list and the regex above), the diff content (parsed as data only). Jobs run
with the permissions they have today; the tree-equality script uses a read-only token. State in one line each how
the change keeps these properties.

Your last message must be a final report. Do not end on a tool call.
