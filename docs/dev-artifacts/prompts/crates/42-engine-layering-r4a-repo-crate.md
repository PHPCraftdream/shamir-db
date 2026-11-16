# Brief: R4a — the `repo` layer becomes the crate `shamir-engine-repo` (ENGINE-LAYERING R4, first extraction)

Design: `docs/dev-artifacts/design/engine-layering.md`, sections 3, 4 (target crate layout, the `shamir-engine-repo` row), 6.2,
7.4 (R4), 9.1 to 9.6 and the "As built" section 11 (read 11.1 to 11.4 first). Landed stages so far: R1, R2a to R2e, R3, R3b
and R2d (see the section 11 list and `git log`). Read also `.rust-guard/layers.toml`, `.rust-guard/layers_baseline.toml`,
`.rust-guard/stages.toml`, `tools/rust-guard/README.md` (the delta file and the layers gate) and the stage briefs
`docs/dev-artifacts/prompts/crates/38-*` to `41-*`.

## Why

`repo` is the lowest layer of `shamir-engine` and no longer refers to any upper layer except three public re-exports in
`repo/mod.rs`, `repo/seed/mod.rs` and `repo/seed/install/mod.rs` that point at the shell (`repo/instance`). Cutting it out
as its own crate gives a unit that compiles and is checked separately; later steps cut `table` (R4b) and `tx` (R4c). This
step changes no behaviour and keeps every public path of `shamir_engine::*` resolving.

## What

1. **Census first (read-only), then act.** List every production and test file under
   `crates/engine/shamir-engine/src/repo/` outside `repo/instance/`. For each file decide: moves to the new crate, or stays
   in the facade. A file stays in the facade only if it must name an upper-layer item (a test that builds a `RepoInstance`
   or uses `table`/`tx`/`query` items cannot compile in the lower crate): such TESTS move to a facade test folder next to
   the `repo/instance` tests (keep the topic layout, at most 7 entries per folder). Production files must have no such need
   (the layers gate says so); if one has, stop and say so in the report instead of inventing a port.
2. **New crate** `crates/engine/shamir-engine-repo` (package `shamir-engine-repo`, `publish = false`, manifest shape of
   `shamir-engine-query/Cargo.toml`: edition, license, `[lib] doctest = false`, `[lints] workspace = true`). Dependencies are
   exactly what the moved tree uses (read the `use` lines: `shamir-storage`, `shamir-wal`, `shamir-tx`, `shamir-types`,
   `shamir-collections`, `shamir-index` and the usual third-party crates; never `shamir-engine`; `shamir-engine-query` only
   if the tree really needs it). Features: `fjall` forwarded to the storage backend crate exactly as the engine does today
   (the `BoxRepo` variants live in this tree), `test-util` if the moved tree has `cfg(feature = "test-util")` items; the
   facade's `fjall`/`test-util` forward to the new crate. `cargo check` writes `Cargo.lock`; do not edit it by hand.
3. **Move** the module trees WITH their `tests/` dirs (git renames, no content rewrite beyond paths): the new crate's
   `src/lib.rs` is `pub mod ...;` declarations and re-exports only, mirroring today's `repo/mod.rs` minus the shell. Items
   that were `pub(crate)` and are used by the facade become `pub` (a cross-crate need); keep every such widening minimal and
   list each one in the final report (they are declared in the campaign delta from the CI findings). Replace `crate::repo::`
   by `crate::` inside the moved tree, and `crate::meta::`-style aliases by the real paths.
4. **Facade shim.** `crates/engine/shamir-engine/src/repo/mod.rs` becomes the re-export facade the design describes in 7.4:
   re-exports of the moved modules from `shamir_engine_repo` plus `pub mod instance;` and the instance re-exports that stay
   as they are today (`RepoInstance`, `repo_instance`, `repo_config`, `fk_reverse_cache`, `version_provider` items,
   `RenamePersistOutcome`, `begin_migration_freeze{,_admitted}`, `seed::prepare_seed`, `seed::install::{SeedImageWriter,
   SeedTable}`, `seed_install::{SeedInstallState, SeedReceipt}`). Every `crate::repo::...` path used by the rest of the engine
   and every `shamir_engine::repo::...` path used by `shamir-db`, `shamir-server*`, `shamir-sync`, the benches and the
   integration tests must keep resolving; prove it with a workspace grep over `crates/` (do not filter paths with
   `grep -v "^crates/..."`, Windows prints backslashes) and with `cargo check` of all dependants. The shim files obey the repo
   rules (`mod.rs` re-exports only; a nested shim module may be a separate file).
5. **rust-guard.** `.rust-guard/stages.toml`: replace the campaign by `engine-layering-R4a` with `packages =
   ["shamir-engine", "shamir-engine-repo"]` and delta files `engine-layering-R4a.toml` and `-discovery.toml` that contain
   ONLY `base = "<sha of the origin/master tip you branched from>"` (do not hand-write entries; the orchestrator generates
   them from the CI findings, with cross-package `[[moved]]` entries). Update path references in `.rust-guard/*.toml`
   (`exceptions.toml`, `owned_impl.toml`, `layers.toml` module bindings, `atomic_orderings.toml`) that name moved files; the
   layers gate configuration of `shamir-engine` keeps working for what stays (the shim `repo/mod.rs` and `repo/instance`);
   lower `.rust-guard/layers_baseline.toml` only if you can show by reading the gate source that a count dropped, and say
   so. There is no previous campaign to delete if `stages.toml` is empty on your base; otherwise delete its delta files.
6. **Registries.** `CLAUDE.md` and `AGENTS.md` crate table and count (the engine group gets `shamir-engine-repo`; the count
   goes from 39 to 40), `scripts/test.sh` scopes (`@engine`, `@oracle` gain the package), and any script or Python test under
   `scripts/ci/` that enumerates engine packages or the `engine-features` expectations
   (`scripts/ci/gates/features_matrix_check.py` and its tests): read them, change what the new crate requires, and state
   each change. DO NOT edit `.github/workflows/*`: list in the final report, per job (`rustdoc`, `engine-features`,
   `engine-features-fast`, doctest, `test-discovery`, `loom-model`), the exact lines that need a workflow edit; the
   orchestrator makes them.
7. **No behaviour change.** No test body, name or `#[ignore]` changes (a moved test keeps its name). No version change.

## Method notes (from the earlier waves, binding)

* Grep the whole `crates/` tree for every moved path before you finish; fix the dependants you find, including benches
  (`crates/engine/shamir-engine/benches/`), `examples/`, `tests/` and `shamir-db`, `shamir-sync`, `shamir-server*`.
* No function-local `use`, no inline test modules, in every file you touch (the structure gate flags old ones too).
* The layers gate follows re-export chains to the real definition: a re-export in a lower layer of an item that lives in a
  higher layer is a backward edge. The shim must not create one that is not already in the baseline.
* Rustdoc links of the form `[`Self::x`]` or `[`crate::repo::x`]` that point at moved items break the rustdoc gate across a
  crate boundary: replace with plain backticks or the new full path.
* Visibility: a moved `pub(crate)` item that the facade uses becomes `pub`; `#[doc(hidden)]` where the item is not meant as
  public API. Do not add `#[allow]`.
* `cfg(test)` items of the moved tree that tests in the facade use must be reachable through a `test-util` feature or a
  `#[doc(hidden)] pub` seam; state each in the report.
* The folder limit is 7 entries and the file limit 1000 lines for every new or changed folder and file; run
  `python -I scripts/ci/check_layout.py`.

## Acceptance (observed in CI)

fmt, clippy (`-p shamir-engine-repo -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server --all-targets`) and layout
clean locally; in CI the layers gate (`repo -> instance` baseline unchanged or lower), rust-guard conservation with the
cross-package delta, test discovery of the moved tests, rustdoc, and the engine and dependant test matrices green. State in
the final report what you could not verify locally.
