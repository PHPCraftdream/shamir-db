# Brief: R4b — the `table` layer becomes the crate `shamir-engine-table` (ENGINE-LAYERING R4, second extraction)

Design: `docs/dev-artifacts/design/engine-layering.md`, sections 3, 4 (target crate layout, the `shamir-engine-table` row),
7.4 (R4), 9.1 to 9.6 and the "As built" section 11. R4a landed: `shamir-engine-repo` holds the `repo` layer and
`crates/engine/shamir-engine/src/repo/` is a facade (`mod.rs` re-exports plus `instance/`, the seed shims and the facade test
folders). Read the R4a brief `docs/dev-artifacts/prompts/crates/42-engine-layering-r4a-repo-crate.md`, the landed R4a diff
(`git log -1 --stat` of the R4a merge and `git show` of the files you need), `.rust-guard/layers.toml`,
`.rust-guard/layers_baseline.toml`, `.rust-guard/stages.toml` and `tools/rust-guard/README.md` (the delta file, the
`platform_gated` flag of the discovery delta and the layers gate).

## Why

`table` is the second layer of `shamir-engine` (`repo < table < validator < tx < query < instance < db_instance`) and the
layers baseline holds no backward edge out of it. Cutting it out as its own crate on top of `shamir-engine-repo` gives the
next unit that compiles and is checked separately; R4c then cuts `tx`. This step changes no behaviour and keeps every public
path of `shamir_engine::*` resolving.

## What

1. **Census first (read-only), then act.** List every production and test file under
   `crates/engine/shamir-engine/src/table/`. For each file decide: moves to the new crate, or stays in the facade. Map every
   `crate::...` path the tree uses: `crate::repo::...` becomes `shamir_engine_repo::...`; the `lib.rs` aliases
   (`crate::index`, `crate::meta`, `crate::migration`, `crate::function` and the like) become the real crate paths; a path
   into `crate::validator`, `crate::tx`, `crate::query`, `crate::db_instance` or `crate::repo::instance` is an upper layer.
   A production file must have no upper-layer need (the layers gate says so; if one has, stop and say so in the report
   instead of inventing a port). A TEST that needs an upper-layer item (a `RepoInstance`, a transaction, a query, the
   validator) cannot compile in the lower crate: it moves to a facade test folder in `shamir-engine` with the topic layout
   kept (at most 7 entries per folder), declared from a facade `mod.rs` so it stays reachable.
2. **New crate** `crates/engine/shamir-engine-table` (package `shamir-engine-table`, `publish = false`, manifest shape of
   `crates/engine/shamir-engine-repo/Cargo.toml`). Dependencies are exactly what the moved tree uses (read the `use` lines;
   `shamir-engine-repo` yes, `shamir-engine` never). Features: forward `fjall` and `test-util` exactly as far as the moved
   tree has `cfg(feature = ...)` items, and make the facade's features forward to the new crate. `cargo check` writes
   `Cargo.lock`; do not edit it by hand.
3. **Move** the module trees WITH their `tests/` dirs (git renames, no content rewrite beyond paths): `src/lib.rs` is
   `pub mod ...;` declarations and re-exports only, mirroring today's `table/mod.rs`. Items that were `pub(crate)` and are
   used by the facade or by the tests that moved to the facade become `pub` (a cross-crate need); keep every widening
   minimal, add `#[doc(hidden)]` where the item is not public API, and list each one in the final report.
4. **Facade shim.** `crates/engine/shamir-engine/src/table/mod.rs` becomes a re-export facade of `shamir_engine_table`
   (plus the facade test folder). Every `crate::table::...` path used by the rest of the engine and every
   `shamir_engine::table::...` path used by `shamir-db`, `shamir-server*`, `shamir-sync`, the benches, `examples/` and the
   integration tests must keep resolving; prove it with a workspace grep over `crates/` (do not filter paths with
   `grep -v "^crates/..."`, Windows prints backslashes) and with clippy of all dependants.
5. **rust-guard.** `.rust-guard/stages.toml`: replace the `engine-layering-R4a` campaign by `engine-layering-R4b` with
   `packages = ["shamir-engine", "shamir-engine-table"]` and delete the R4a delta files. The R4b delta files
   `.rust-guard/stages/engine-layering-R4b.toml` and `engine-layering-R4b-discovery.toml` contain ONLY
   `base = "<sha of the origin/master tip you branched from>"`; the orchestrator generates the entries from the CI findings.
   Update path references in `.rust-guard/*.toml` (`exceptions.toml`, `owned_impl.toml`, `atomic_orderings.toml`) that name
   moved files. `layers.toml`: a crate with a single layer gets no entry (R4a learned that a one-layer crate entry fails the
   gate with `LAYER_UNASSIGNED`); `shamir-engine` keeps its layer list for what stays. Lower `layers_baseline.toml` only if
   you can show from the gate source that a count dropped.
6. **Registries.** `CLAUDE.md` and `AGENTS.md` crate table and count (the engine group gets `shamir-engine-table`; the
   count goes from 40 to 41), `scripts/test.sh` scopes (`@engine`, `@oracle`), and every script or Python test under
   `scripts/ci/` that enumerates engine packages or feature expectations (`scripts/ci/gates/features_matrix_check.py` and its
   tests: R4a added the `shamir-engine-repo` row, follow it). DO NOT edit `.github/workflows/*`: list in the final report,
   per job (`rustdoc`, `engine-features`, `engine-features-fast`, doctest, `test-discovery`, `loom-model`), the exact lines
   that need a workflow edit; the orchestrator makes them (R4a's lines for `shamir-engine-repo` are the model).
7. **No behaviour change.** No test body, name or `#[ignore]` change (a moved test keeps its name). No version change.

## Lessons from R4a (binding)

* **A `cfg(test)` switch does not reach a dependency.** Test-only seams of the moved tree that facade tests use are read
  under `cfg(any(test, feature = "test-util"))` (attribute or `cfg!`), and the facade's dev-dependency enables `test-util`.
  Never put such a gate on an item that production code calls: R4a gated `merge_store_names`, which production calls, and
  the crate did not compile without `test-util`.
* **`--all-targets` hides feature bugs.** With `--all-targets` the dev-dependencies switch `test-util` on for the library
  too. Besides the `--all-targets` clippy, run `cargo clippy -p shamir-engine-table --lib -- -D warnings` once without
  features and once per feature and feature combination the crate declares, and report each result.
* **Count folders, do not copy counts.** Every folder you add to or regroup is counted from a listing, not from a document.
* **Platform-gated tests.** A moved test under `cfg(unix)`, `cfg(windows)` or a feature that a CI cell lacks is absent from
  that cell's discovery list; list every such moved test in the report so the orchestrator marks its discovery entry
  `platform_gated = true`.
* **Rustdoc across the crate boundary.** Intra-doc links to `crate::table::...`, `crate::repo::...` or upper-layer items break
  the rustdoc gate after the move: use the new full path or plain backticks.

## Method notes (binding)

* Grep the whole `crates/` tree for every moved path before you finish; fix the dependants you find.
* No function-local `use`, no inline test modules, in every file you touch (the structure gate flags old ones too).
* The layers gate follows re-export chains to the real definition: the shim must not create a backward edge.
* No inherent `impl` of a type of the new crate inside the facade (it will not compile across crates): use a free function
  or an extension trait in the facade.
* At most 7 entries per folder and 1000 lines per file for every new or changed folder and file; run
  `python -I scripts/ci/check_layout.py`.

## Acceptance (observed in CI)

fmt, clippy (`-p shamir-engine-table -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server --all-targets`, plus the
lib-only feature cells of the new crate) and layout clean locally; in CI the layers gate, rust-guard conservation with the
cross-package delta, test discovery of the moved tests, rustdoc, and the engine and dependant test matrices green. State in
the final report what you could not verify locally.
