You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
byte-meter-1891). Read the root CLAUDE.md (binding) and `tools/rust-guard/README.md`. The brief below is the task; it
is committed at docs/dev-artifacts/prompts/crates/32-byte-meter-dedup.md.

# Brief: one formula for the result byte estimate (BYTE-METER-DEDUP)

## Why

The batch byte budget (a DoS guard: it stops a batch whose materialised result grows past a limit) estimates the size
of a `QueryValue` with a fixed formula. Since the engine layering wave R1 that formula exists TWICE:
`crates/engine/shamir-engine/src/query/batch/runner/result_size.rs` (`ResultSize::value`) and
`crates/engine/shamir-engine-query/src/query/read/row_byte_meter.rs` (`RowByteMeter::value_size` plus a private
`sequence_walk`, a copy of the walk "so the leaf does not depend on the batch crate"). Their synchrony is held by a
comment ("the constants must change together"); a divergence would silently skew the budget. A second type with the
same name `RowByteMeter` also exists in `crates/engine/shamir-engine/src/query/batch/exec/exec_scope.rs` (a scope
meter, a different thing) and the name collision already broke an import once.

## What (the numbers the estimate produces must not change)

1. ONE implementation of the value walk, in the leaf crate `shamir-engine-query` (it owns `QueryValue` handling and
   is below the engine). `ResultSize::value` (and every other user in the engine) delegates to it. Delete the copy.
2. Give the two meters distinct names that say what they are (for example the leaf one a function or a type named
   after the estimate, the scope one named after the per-scope accounting), update every user (grep `RowByteMeter`,
   `ResultSize`, `value_size`, `sequence_walk` in `crates/` including tests and benches), keep any public path other
   crates use (check `shamir-db`, `shamir-server` with grep before renaming a `pub` item; if a public item must keep
   its old name, re-export).
3. Add a pin test in the leaf crate (module `tests/` folder of the read module, plain `#[test]` fns, multi-line
   literals): a table of values and their exact expected estimates, covering Null, Bool, Int, F64, Dec, Big, Str,
   Bin, a nested List, a Set, a Map with keys, an empty container and a deeply nested value (the walk is iterative:
   assert a 100,000-deep chain does not overflow the stack). Take the expected numbers from the CURRENT formula
   (scalar slot 16; string/bin 24 + len; decimal 56; big integer 24 + decimal text length; container shell
   24 + 16 per element/entry plus key lengths for maps; plus the fixed 48 per-row shell in `value_size`), by
   reading the code, not by guessing.
4. The existing batch byte-budget tests keep passing unchanged; do not weaken them.

## Scope and siblings

Files in `shamir-engine-query/src/query/read/`, `shamir-engine/src/query/batch/runner/result_size.rs`,
`.../batch/exec/exec_scope.rs` and their direct users. Other agents work in parallel in other places (the layers gate in
`tools/rust-guard`, file splits in other crates, the `tx` and `repo` layers of the engine): do not touch those. The
batch `exec/batch_execute.rs` and `exec/interactive_tx.rs` are edited by a LATER stage of the engine layering; change
them only if a rename forces a one-line edit.

## Observable acceptance

`cargo fmt` and `cargo clippy -D warnings --all-targets` clean for `-p shamir-engine-query -p shamir-engine
-p shamir-db`; layout passes; the final report lists every renamed/moved/changed item. CI observes the rust-guard
gates (the orchestrator fills a campaign delta from the findings), rustdoc, engine-features, test-discovery and the
full matrix. Non-goals: any change of the estimate, any other batch behaviour.

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
