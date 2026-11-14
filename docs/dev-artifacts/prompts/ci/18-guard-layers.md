You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
guard-layers-1871). Read the root CLAUDE.md (binding), `tools/rust-guard/README.md` and the existing subcommands
under `tools/rust-guard/src/`. The brief below is the task; it is committed at
docs/dev-artifacts/prompts/ci/18-guard-layers.md.

# Brief: `rust-guard layers` — no new backward dependencies between modules of a crate (GUARD-LAYERS)

## Why

Cargo forbids dependency cycles between crates, but `rustc` allows any module of a crate to use any other, so
`shamir-engine` (62k production lines) grew cycles between its top-level modules `table`, `tx`, `query`, `repo`,
`db_instance`, `validator`. They block splitting it into crates. A one-off clean-up would rot unless a gate keeps
new cycles out. Measured against the candidate order `repo` < `table`,`validator` < `query` < `tx` <
`repo/instance` < `db_instance` there are about 53 production files that point "upward" (table->query 40,
query->tx 5, table->db_instance 3, table->tx 2, validator->query 2, repo->table 1), and `RepoInstance` (the
composition root in `repo/instance`) is referenced from outside it by 40 files in `tx`, 7 in `query`, 6 in `table`.

## What

1. **Prerequisite, no behaviour change:** `tools/rust-guard/src` already has exactly 7 entries (the folder limit).
   Group the subcommand folders (for example under `commands/`) so that a new subcommand fits; keep the CLI
   unchanged and every existing self-test passing.
2. **Subcommand `rust-guard layers`.** Builds the module-dependency graph of each configured crate and compares it
   with a configured layering. Edge `A -> B` = production code of top-level module (or configured sub-module such as
   `repo/instance`) `A` refers to module `B` of the same crate. Count every way of referring, or the gate is
   trivially bypassed:
   * `use crate::b::..`, including grouped `use crate::{..}` and `use` of a re-exported path;
   * paths in code without `use` (`crate::b::Foo::new()`);
   * `super::` chains that leave the module (`super::super::b`);
   * re-exports at the crate root (`pub use repo::meta;` in `lib.rs` makes `crate::meta::X` an edge into `repo`);
   * `impl Type` / `impl Trait for Type` where `Type` is owned by another module (`impl RepoInstance` written in
     `tx` is an edge `tx -> repo/instance`).
   Test code (`tests/` folders, `cfg(test)`) is collected separately and never fails the gate.
3. **Config `.rust-guard/layers.toml`:** per crate an ordered list of layers (lowest first); a module may refer to
   its own layer and lower ones only. **Ratchet baseline** in the same file (or a sibling): the current backward
   edges with a file count per ordered pair, like `scripts/ci/check_layout.py`'s baseline: a new backward pair
   fails, a known pair whose count grows fails, counts may only go down, `--update-baseline` only lowers or removes
   entries. Each failure prints `path:line: LAYER_BACKWARD: <A> -> <B>` in the tool's existing finding format with a
   hint naming the allowed direction.
4. Configure `shamir-engine` with the PRELIMINARY order above and record its baseline. (The final order comes from a
   separate design task; this config must be trivially editable.)
5. Wire into CI as a step of the `source-integrity` job (`.github/workflows/ci.yml`) and into `rust-guard
   self-test`; document it in the rust-guard section of the root `CLAUDE.md` (one short paragraph) and in
   `tools/rust-guard/README.md`.
6. **Self-tests with fixtures** (the tool already has a `tests/` dir): one fixture per kind of edge above including
   the `impl` and re-export cases, a clean crate, a baseline that must not grow, a baseline entry that dropped to
   zero (must be pruned or reported as stale, like `STALE_EXCEPTION`).

## Observable acceptance

`cargo fmt` / `clippy -D warnings` clean for the rust-guard crate (a workspace-excluded binary: use
`--manifest-path tools/rust-guard/Cargo.toml`); layout passes; the CI `rust-guard self-test` and `source-integrity`
jobs pass (the orchestrator observes) and the baseline matches the numbers above within a small tolerance (state any
difference and why in your report). Non-goals: changing any engine source; enabling the gate for other crates.

## Constraints (binding)

Compilation, builds and tests run in CI, run by the orchestrator, never on this machine. Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build/run/bench/doc`, `npm`/`tsc`/`vitest`, wasm builds, e2e and
integration harnesses, loom, any release or full-workspace build, any command whose purpose is to build the project
or run its tests. Allowed locally, scoped to the crates you touched and nothing else:
`cargo fmt -p <crate>` (and `-- --check`), `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git (`status`, `diff`, `log`). Run cargo only under
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
