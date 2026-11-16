# Brief: R2d — `query/batch` takes `RepoCore`, not `RepoInstance` (ENGINE-LAYERING R2d)

## Why

R2c2 (the parent commit) moved the whole `tx` layer onto `RepoCore`; `tx -> instance` is zero. `query/batch` still names
`RepoInstance` in 8 production files (`rust-guard layers` counts 11 files with a `query -> instance` edge, baseline
`.rust-guard/layers_baseline.toml`): `exec/executor_traits.rs` (`TableResolver::resolve_repo` returns `RepoInstance`),
`exec/interactive_tx.rs`, `control/receipts/commit.rs`, `control/implicit.rs` (the implicit-batch bodies of R2c1b), the
four `fk` files, and the files that call `begin_tx`, `commit_tx*`, `fk_reverse_cache`, `list_table_names` through the
shell. That edge must go before R4 (the layers become crates). Behaviour must not change.

The original design (`docs/dev-artifacts/design/engine-layering.md` 5.6, 5.7, 7.2 "R2d") proposed a commit port trait
(`BatchTxPort`). Orchestrator decision (2026-11-15): do NOT introduce a port trait or a `dyn` on the commit, read or write
path. `query` is ABOVE `tx` in the layer order, so batch code may call the `tx` layer directly; after R2c2 every `tx`
function takes `&RepoCore`. Use `RepoCore` (a shallow, cheap clone) in batch code. A port is allowed only for a callee that
genuinely stays in the `instance` layer, and then you must say which and why in the report.

## What

1. **Census first (read-only).** List every production item under `crates/engine/shamir-engine/src/query/` (paths without
   `tests`) that names `RepoInstance`, `crate::repo::instance`, `repo_instance` or calls a shell-only method. For each shell
   method the batch code calls (`begin_tx`, `commit_tx`, `commit_tx_with_options`, `fk_reverse_cache`, `list_table_names`,
   `get_table*`, `tx_gate`, `drainer`, ...) check whether it already exists on `RepoCore` (R2c1b and R2c2 moved most of
   them); if not, first move its body onto `RepoCore` (inherent impl in `tx/commit/core/`, one-line delegate left on the
   shell, same signature) when its body names only `repo`, `table`, `validator` and `tx` items. `tx/commit/core/` is at its
   entry limit: add to the existing topical files or split one into a subfolder; at most about 400 lines per file.
2. **`resolve_repo` returns `RepoCore`.** `TableResolver::resolve_repo` (`executor_traits.rs`) changes its return type from
   `DbResult<RepoInstance>` to `DbResult<RepoCore>`. `RepoCore` is `pub` inside a `pub(crate)` module today, so external
   implementors cannot name it: add a documented public re-export of `RepoCore` from `tx/mod.rs` (the path
   `shamir_engine::tx::RepoCore`), nothing else widens. Every implementor changes with it: `DbTableResolver` in `shamir-db`
   (`execute/core/table_resolver.rs`) and every resolver in benches and tests of `shamir-engine` (grep `impl TableResolver
   for` over the whole workspace, `crates/`, including `tests/` and `benches/` directories and the other server crates).
3. **Batch bodies.** Convert `&RepoInstance` / `RepoInstance` in the batch files to `&RepoCore` / `RepoCore`, including
   `control/implicit.rs` (its four free functions take `&RepoCore`; the shell delegates in
   `repo/instance/repo_instance/transactions.rs` pass `&self.core`). The fk files take the core as well.
4. **Callers.** Production callers outside `query` that pass a `RepoInstance` to a changed function pass `repo.core()` /
   `&self.core`. Tests that call a changed function directly pass the core; the shell delegates stay, so tests through
   `repo.commit_tx(..)` need no edit. No test body, assertion, name or `#[ignore]` state changes; no test is removed.
5. **Find EVERY call site before you finish.** The previous stage needed four CI rounds because call sites outside
   `shamir-engine` were missed. Search the whole `crates/` tree with `grep -rn` (the Windows paths use backslashes: do not
   filter with `grep -v "^crates/..."`), then run the allowed local check on every dependent:
   `cargo check -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server --all-targets` and
   `cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings` (cargo only under `wnice`, `-j 2`, one cargo
   process at a time). `cfg(unix)`, `cfg(feature = ...)` and `cfg(loom)` code is not compiled on this machine: re-read it by
   hand.
6. **Structure gate pitfalls (learned in R2c2).** The structure gate flags EVERY function-local `use` and every inline
   `#[cfg(test)] mod x { ... }` with bodies in EVERY file your change touches, old code included: before you finish, grep each
   changed file for `^\s+use ` and hoist the imports to the file header; move any inline test module to its own file. Intra-doc
   links of the form ``[`x`](Self::x)`` to an item that moved are errors in the CI rustdoc gate: use plain backticks and
   re-read every doc comment you touch.
7. **Layers ratchet.** Lower `.rust-guard/layers_baseline.toml` to the real count: when no production file under `query/`
   names the instance layer any more, delete the `"query -> instance"` line (the file's header allows only lowering); if you
   cannot reach zero, set the number you achieved and say which files remain and why.
8. **Campaign.** Replace the R2c2 entry `engine-layering-R2c2` in `.rust-guard/stages.toml` and delete its two delta files
   (`.rust-guard/stages/engine-layering-R2c2.toml`, `...-discovery.toml`). Add `engine-layering-R2d`
   (`packages = ["shamir-engine"]`, `delta = ".rust-guard/stages/engine-layering-R2d.toml"`, `discovery_delta =
   ".rust-guard/stages/engine-layering-R2d-discovery.toml"`) and the two delta files containing ONLY
   `base = "<the sha of this branch's merge base with origin/master>"`. Do NOT write delta entries (the orchestrator fills
   them from the CI findings).
9. **Do not** change behaviour, create crates, touch versions, or change public paths other than the one re-export of item 2
   (`shamir_engine::{repo,table,tx,query,validator,meta,migration}::*` and everything `shamir-db`, `shamir-server*` and
   `shamir-sync` import must still resolve).

## Layer rules

`repo < table < validator < tx < query < instance < db_instance`; a module may refer only to its own and lower layers.
After this stage no production file under `query/` may name `crate::repo::instance`, `RepoInstance` or `crate::db_instance`.
A `repo/mod.rs` re-export of a `table`, `tx` or `query` item is a backward pair: add none.

## Observable acceptance

`cargo fmt -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server -- --check`, the clippy and check commands of item 5
clean; `python -I scripts/ci/check_layout.py` OK; `grep -rw RepoInstance` over the production files under `query/` finds
nothing but comments you rewrote; every public path unchanged except the one re-export. The final report lists every
converted function, every method moved onto `RepoCore`, every implementor of `TableResolver` you changed, every call site
outside `query`, every widened visibility, the count of `query -> instance` files you reached, and what must be observed in
CI. Non-goals: the `repo -> instance` edges (R2e), crate creation, behaviour change, version bumps.
