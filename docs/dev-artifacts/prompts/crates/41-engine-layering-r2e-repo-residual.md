# Brief: R2e — the `repo` layer stops referring to the `instance` layer (ENGINE-LAYERING R2e)

## Why

`rust-guard layers` still counts 5 production files with a `repo -> instance` edge (baseline
`.rust-guard/layers_baseline.toml`). The layer `repo` becomes the crate `shamir-engine-repo` at R4 and cannot depend on
`instance` (the facade crate above it). No earlier stage owned these files; R2b and R3 fixed part of them. Behaviour must not
change.

## What

1. **Census first (read-only).** Find the 5 files: list every production file under `crates/engine/shamir-engine/src/repo/`
   OUTSIDE `repo/instance/` (paths without `tests`) that names `RepoInstance`, `crate::repo::instance`, `repo_instance`,
   `super::instance` or an item that lives under `repo/instance/`. Known sites on the parent commit (verify, do not trust):
   - `repo/journal/backend/durability_policy/durability_policy/commit_policy.rs`: `commit_wal_durability(repo: &RepoCore)` -
     R2c2 changed `&RepoInstance` to `&RepoCore`, which is a `repo -> tx` edge (RepoCore is in `tx`): check whether the layers
     gate saw it; either way the function must take what the `repo` layer owns: `&DurabilityHandle` (and the repository name
     as a `&str` for its `cfg(test)` seam `TEST_SYNCED_WAL_REPO`), and its callers in `tx` (`tx/commit/pipeline/phase/commit/
     legacy_and_lockfree.rs`, `tx/preparation/pre_commit/locked.rs`, and the re-export chain through `tx/commit/mod.rs`) pass
     `&repo.durability_handle` / `repo.name()`. Put no `repo -> tx` edge in its place.
   - `repo/journal/backend/durability_policy/mod.rs`: the `#[cfg(test)]` re-export of
     `crate::repo::instance::repo_instance::checkpoint_allocation_floor`: point the test users (`tx/recovery/clean_horizon/
     checkpoint.rs` seam tests) at the real owner instead.
   - `repo/seed/mod.rs` and `repo/seed/install/mod.rs`: compatibility re-exports of `prepare_seed`, `SeedImageWriter`,
     `SeedTable` from `repo::instance::seed` (R3 moved the seed orchestration up and left shims so that the public paths
     `shamir_engine::repo::seed::...` keep resolving), and `repo/mod.rs` (re-exports of `RepoInstance` and aliases).
2. **Remove each edge by moving code or its owner, not by hiding it.** Prefer moving the item down to the layer it belongs
   to. A re-export in a lower layer of an item that lives in a higher layer IS the edge; relocating the shim does not remove
   it. For a public path that can only be served by such a re-export until the facade exists, read
   `docs/dev-artifacts/design/engine-layering.md` (5.9, 8.2, 8.3, 9.1 "Public API / facade re-export table") and decide per
   site: (a) the path can be served from a place outside the layers (the crate root `lib.rs` is not a layer), or (b) the item
   itself can move down, or (c) the edge stays until R4. For (c), do NOT hack: leave the edge, lower the baseline only to what
   you reached, and say in the report which files remain, the public path each one carries, and why R4 resolves it.
3. **Find every call site.** `grep -rn` the whole `crates/` tree (Windows paths use backslashes: do not filter with
   `grep -v "^crates/..."`) for each function or path you change, including `tests/` and `benches/` directories and the
   other crates. Local checks on dependents (cargo only under `wnice`, `-j 2`, one cargo process at a time):
   `cargo check -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server --all-targets` and
   `cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings`. `cfg(unix)`, `cfg(feature = ...)` and
   `cfg(loom)` code is not compiled on this machine: re-read it by hand.
4. **Structure gate pitfalls (learned in R2c2).** The structure gate flags EVERY function-local `use` and every inline
   `#[cfg(test)] mod x { ... }` with bodies in EVERY file your change touches, old code included: before you finish, grep each
   changed file for `^\s+use ` and hoist the imports to the file header. Intra-doc links of the form ``[`x`](Self::x)`` to
   an item that moved are errors in the CI rustdoc gate: use plain backticks and re-read every doc comment you touch.
5. **Layers ratchet.** Lower `.rust-guard/layers_baseline.toml` to the real count: delete the `"repo -> instance"` line when
   you reach zero, otherwise set the number you reached.
6. **Campaign.** Replace the R2c2 entry `engine-layering-R2c2` in `.rust-guard/stages.toml` and delete its two delta files
   (`.rust-guard/stages/engine-layering-R2c2.toml`, `...-discovery.toml`). Add `engine-layering-R2e`
   (`packages = ["shamir-engine"]`, `delta = ".rust-guard/stages/engine-layering-R2e.toml"`, `discovery_delta =
   ".rust-guard/stages/engine-layering-R2e-discovery.toml"`) and the two delta files containing ONLY
   `base = "<the sha of this branch's merge base with origin/master>"`. Do NOT write delta entries (the orchestrator fills
   them from the CI findings). If R2d (a parallel stage on `query/batch`) lands first the orchestrator reconciles the two
   campaign entries; keep your change to the files named above.
7. **Do not** touch `query/` (R2d owns it), change behaviour, create crates, change any public path that cannot be served
   another way (see item 2), or touch versions.

## Layer rules

`repo < table < validator < tx < query < instance < db_instance`; a module may refer only to its own and lower layers. After
this stage no production file under `repo/` outside `repo/instance/` may name the instance layer (or the report says which
and why). A change must not create a `repo -> tx` or `repo -> table` edge.

## Observable acceptance

`cargo fmt -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server -- --check`, the check and clippy commands of item
3 clean; `python -I scripts/ci/check_layout.py` OK; every public path unchanged. The final report lists the 5 files with
what you did to each, every function whose signature changed, every call site, every widened visibility, the count you
reached, and what must be observed in CI. Non-goals: `query/`, crate creation, behaviour change, version bumps.
