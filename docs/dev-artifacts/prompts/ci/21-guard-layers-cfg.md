You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
guard-cfg-1889). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and the `layers` command under
`tools/rust-guard/src/commands/layers_cmd/` (`graph.rs`, `refs.rs`, `run.rs`, `config.rs`) and its self-tests under
`tools/rust-guard/tests/gates/layers_tests.rs` with fixtures in `tools/rust-guard/tests/fixtures/layers/`. The brief
below is the task; it is committed at docs/dev-artifacts/prompts/ci/21-guard-layers-cfg.md.

# Brief: the layers gate must see production code under `cfg(not(test))` (GUARD-LAYERS-CFG)

## Why (a bug in the gate that landed a few days ago)

`cfg_mentions_test` in `layers_cmd/graph.rs` treats an item as test code when its `cfg(...)` predicate MENTIONS the
identifier `test` or `loom` anywhere (`expr_mentions_test` recurses through calls, parens, assignments and tuples and
looks only at the identifiers, never at `not`). So production code under `#[cfg(not(test))]` or `#[cfg(not(loom))]`
is skipped by the gate: a backward dependency can hide there. In `shamir-engine` there are about 14 such sites
(for example `repo/instance/repo_instance/catalog/rename_freeze.rs:9`, `.../lifecycle/durability_repo_impl.rs:64`,
`repo/instance/seed/install/writer.rs`, `repo/journal/changelog_store/write_domain.rs:139`, the production branch of
`WriterDrainBarrier`). `is_test_name` additionally skips a module by its NAME alone (`tests`, `*_tests`, `*_loom`)
without looking at the declaration's `cfg`. The same class of bug was fixed before in the CI matrix tooling
(`scripts/ci/matrix/checks.py`, `_attrs_before`: `cfg(test|loom)` without `not` is test-only).

## What

1. A `cfg` predicate marks code as test-only only when it REQUIRES `test` or `loom`: `test`, `loom`,
   `all(.., test, ..)` / `all(.., loom, ..)` (any conjunct requiring it), and `any(..)` whose every alternative
   requires it. `not(test)`, `not(loom)`, `any(not(test), ..)`, `all(not(test), feature = "x")` are PRODUCTION.
   Parse nested `all`/`any`/`not` properly (syn parses `cfg` arguments as `Expr` today: handle `Expr::Call` of
   `all`/`any`/`not` and `Expr::Path` of `test`/`loom`; keep `feature = ".."` and other predicates neutral).
2. A module is test code when the declaration carries a test-only `cfg` OR its file lives in a `tests` directory; the
   name alone (`foo_tests`, `foo_loom`) no longer decides when the declaration has no test cfg and the file is not in
   a `tests/` directory. Read how the graph resolves module files and keep the behaviour for the real tree
   unchanged where the name convention and the folder agree (the repo's rule: tests live in `tests/` folders).
3. Self-test fixtures and tests in the existing style (one fixture per case; exact finding code):
   a backward edge under `#[cfg(not(test))]` fails; under `#[cfg(not(loom))]` fails; under `#[cfg(test)]` does not;
   under `#[cfg(all(test, feature = "x"))]` does not; under `#[cfg(any(test, loom))]` does not;
   under `#[cfg(any(test, feature = "x"))]` fails; a module `foo_tests` declared WITHOUT cfg in a non-`tests` folder
   is production; the same module inside a `tests/` folder is test code.
4. Do not edit `.rust-guard/layers_baseline.toml`: after your fix the gate may report more backward files in
   `shamir-engine` (previously hidden). That is the point; the orchestrator trues the baseline up from the CI output
   in the same pull request. List in your report which hidden sites you expect.
5. Document the exact rule in `tools/rust-guard/README.md` (the `layers` section) in two or three lines.

## Observable acceptance

`cargo fmt --manifest-path tools/rust-guard/Cargo.toml -- --check` and
`cargo clippy --manifest-path tools/rust-guard/Cargo.toml --all-targets --locked -- -D warnings` clean (workspace-
excluded binary, use the manifest path); layout passes (`tools/rust-guard/tests/` and `commands/layers_cmd/` entry
and line limits); the new tests are plain `#[test]` fns in the existing test files. CI observes: `rust-guard
self-test` (the new tests) and `source-integrity` (the `layers` step, expected to name the newly visible edges).
Non-goals: any engine source change, the baseline, other gates.

## Lessons from the earlier waves (binding for this wave)

* **Do not hand-write the rust-guard delta entries.** Hand-derived deltas mismatched the gate in hundreds of places.
  Create the campaign entry in `.rust-guard/stages.toml` and the two delta files
  (`.rust-guard/stages/<name>.toml` and `<name>-discovery.toml`) containing ONLY the line
  `base = "<sha of the origin/master tip you branched from>"` (use `git rev-parse HEAD~0` of your worktree's base: the
  worktree was created from origin/master). The orchestrator fills the entries from the CI findings. Do record
  every item you moved, changed or added in your final report so the orchestrator can cross-check.
* **No function-local `use` in any file you touch** (the structure gate flags every local `use` in a changed file,
  also old ones): hoist them to the file header.
* **Code under `cfg(unix)`, `cfg(windows)` or `cfg(feature = ...)` is not compiled by your local clippy on this
  Windows machine.** Re-read every changed file for unused imports, wrong paths and privacy in such code
  (CI compiles it on Linux with all features).
* **Folder limit: at most 7 direct entries.** Counts today: `table/` 7, `table/support/` 7, `table/support/model/` 7,
  `table/table_manager/` 6, `repo/` 7, `repo/instance/` 7, `repo/instance/repo_instance/` 7, `tx/` 7, `query/batch/` 7,
  `table/support/storage/` 5, `table/support/guards/` 5, `db_instance/` 5. A new module must go to a folder with a
  free slot or you must regroup. `python -I scripts/ci/check_layout.py` is the authoritative check; if the sandbox
  refuses to run it, count entries and lines by hand and say so in the report. At most 1000 lines per file.
* A `mod.rs` that declares a new submodule is the file where the gate reports the new item; every new test module
  must be declared (`#[cfg(test)] mod tests;`) or its files are unreachable.
* Re-exports keep every public path working: `shamir_engine::{repo,table,tx,query,validator,meta,migration}::*` and
  everything `shamir-db`, `shamir-server*` and `shamir-sync` import must still resolve; clippy for
  `-p shamir-db` proves that.
* **No inherent `impl` of a lower-layer type in an upper layer** (for example `impl TableManager` inside
  `repo/instance`): it will not compile once the layers are separate crates. Use a free function or an extension
  trait in the upper layer instead.
* **Bundles expose methods, not public fields.** Do not make the shell reach into
  `self.<bundle>.<field>` for anything but construction; add a method on the bundle.
* **Do not duplicate a formula or a helper to avoid a dependency direction**: move the single implementation to the
  lower layer and call it from above.
* A file that would pass 1000 lines is split in the same change; do not compress lines to fit.

## Constraints (binding)

Compilation, builds and tests run in CI, run by the orchestrator, never on this machine. Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build/run/bench/doc`, `npm`/`tsc`/`vitest`, wasm builds, e2e and
integration harnesses, loom, any release or full-workspace build, any command whose purpose is to build the project
or run its tests. Allowed locally, scoped to the crates you touched and nothing else:
`cargo fmt -p <crate>` (and `-- --check`), `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git (`status`, `diff`, `log`). Do not run the Python test suites either (CI runs them). Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --` with `-j 2`. A change you cannot verify here is verified by the orchestrator's CI
run: say in your final report exactly what must be observed there. If you believe you need a build or test run,
say so in the final report instead of running it.

Repo rules (read the root CLAUDE.md, it is binding): at most 7 entries per folder, at most 1000 lines per source
file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no visibility
widening, `shamir_collections::THasher` for hash maps, comments terse. Never write private machine paths, user
names, or the names of any CI infrastructure other than the workflow files into repository files. Never raise
versions. Do not touch code unrelated to the task.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: what changed (files), what you verified locally (fmt/clippy/layout only),
what is unverified and must be observed in CI, anything you decided and why. Do not end on a tool call.
