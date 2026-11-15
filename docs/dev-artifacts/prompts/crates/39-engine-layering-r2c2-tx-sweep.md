# Brief: R2c2 — the `tx` layer takes `RepoCore`, not `RepoInstance` (ENGINE-LAYERING R2c2)

## Why

After R2c1 stage 2 (the parent commit of this branch) `RepoCore` carries every repository method the `tx` files call, and
`RepoInstance` is the shell `{ name, live, core, rename_fence, rename_fence_len }` with one-line delegates. The `tx`
layer still names `RepoInstance` in 43 production files (`rust-guard layers` counts 35 files with a `tx -> instance`
edge; baseline `.rust-guard/layers_baseline.toml`). That edge must go: `RepoCore` lives in `tx`, `RepoInstance` in
`instance`, and R4 will cut them into different crates. This stage converts the `tx` layer. Behaviour must not change.

## What

1. **Census first (read-only), then convert callees before callers.** List every production item under
   `crates/engine/shamir-engine/src/tx/` (paths without `tests`) that names `RepoInstance`: parameters (`&RepoInstance`,
   `&Arc<RepoInstance>`, `RepoInstance` by value), struct fields (the drainer, the receipt worker and lifecycle jobs,
   cold-reconcile jobs, ...), generic arguments, closures, doc links, `cfg(test)` / `cfg(feature)` / `cfg(loom)` code.
   Build the call graph and convert in dependency order: a function taking `&RepoCore` cannot call one that still takes
   `&RepoInstance`; the directories call each other (`preparation -> commit`, `drain -> commit`, `drain -> recovery`,
   plus calls through the re-exports of `tx/mod.rs`). One sequential stage, not parallel chunks, for the production code.
2. **Type mapping.** `&RepoInstance` and `&Arc<RepoInstance>` become `&RepoCore`. A field or a by-value parameter that
   holds a `RepoInstance` becomes a `RepoCore` (it is a shallow, cheap clone: every field is shared). No `Deref`,
   `AsRef` or `From` bridge between the two types, and no new trait or `dyn` on a commit, read or drain path: the
   conversion is explicit at each boundary (`&repo.core`, `self.core.clone()`).
3. **What the shell must still hand over.** If a `tx` function needs state that lives only on the shell (`live`, the
   rename fence, `clone_for_background`), do not pass the shell: pass the needed value as an explicit parameter, or keep
   that single function on the shell. If a `tx` function still calls a method that exists only on the shell, first move
   that method onto `RepoCore` (an inherent impl in `tx/commit/core/`, one-line delegate left on the shell, same
   signature) when its body names only `repo`, `table`, `validator` and `tx` items; otherwise stop and say so in the
   report. `tx/commit/core/` holds 7 entries: add impl blocks to the existing topical files or split one of them into a
   subfolder; a file stays under about 400 lines.
4. **The drainer.** The `OnceLock::get_or_init` closure that builds the drainer and spawns its task stays installed by the
   SHELL (`repo/instance/repo_instance/lifecycle.rs`): it owns the background clone (`live = None`) and the `Weak` of
   the liveness token and decides when the task exits. The drainer and its task now hold a `RepoCore` taken from that
   background clone; the exit condition, the ordering of the drain steps and the use of the `Weak` do not change. State
   in the report, with file and line, what the closure hands the drainer before and after.
5. **Callers outside `tx`.** Production callers of the converted functions that still hold a `RepoInstance` (the shell in
   `repo/instance/**`, the three `query/batch` files that commit and begin transactions, `db_instance`, `shamir-db`) pass
   `&repo.core` / `self.core` at the call. Keep these edits to the call expression: `query/batch` is rewritten by the next
   stage (R2d), so there change only what must change to compile.
6. **Tests.** Tests that call a converted function directly (not through a shell method) pass `&repo.core`. The shell
   delegates stay, so tests going through `repo.commit_tx(..)` and the like need no edit. Test bodies, assertions, names and
   `#[ignore]` state do not change; no test is removed. `cargo clippy --all-targets` for `shamir-engine` and `shamir-db`
   compiles the test code: use it to find every site. Code behind `cfg(unix)`, `cfg(feature = ...)` and `cfg(loom)` is not
   compiled on this machine: re-read it by hand (grep for the converted names under those cfgs).
7. **Delegation.** One executor does the production sweep. Once the production signatures are final it may hand the test
   call-site edits to at most two workers on disjoint directories (the shared store is contended; a worker that dies on a
   store timeout is resumed, it is not a failure). Workers never run builds or tests and never touch git.
8. **Campaign.** Replace the stage-2 entry `engine-layering-R2c1b` in `.rust-guard/stages.toml` and delete its two delta
   files `.rust-guard/stages/engine-layering-R2c1b.toml` and `...-discovery.toml`. Add the campaign `engine-layering-R2c2`
   (`packages = ["shamir-engine"]`, `delta = ".rust-guard/stages/engine-layering-R2c2.toml"`, `discovery_delta =
   ".rust-guard/stages/engine-layering-R2c2-discovery.toml"`) and the two delta files containing ONLY
   `base = "<the sha of this branch's merge base with origin/master>"`. The orchestrator fills them from the CI findings.
   Do not edit `.rust-guard/layers_baseline.toml` (machine-managed; the orchestrator lowers it from the CI result).
9. **Do not** touch `query/batch` beyond the call expressions of item 5, do not create crates, do not change public paths
   (`shamir_engine::{repo,table,tx,query,validator,meta,migration}::*` and everything `shamir-db`, `shamir-server*` and
   `shamir-sync` import must still resolve), do not change `rename_fence`, do not touch versions.

## Layer rules (the `rust-guard layers` ratchet fails on any new backward pair; you cannot run it here)

`repo < table < validator < tx < query < instance < db_instance`; a module may refer only to its own and lower layers.
After this stage no production file under `tx/` may name `crate::repo::instance`, `RepoInstance`, `crate::query` or
`crate::db_instance`. Re-read every path a converted file names before you leave it. A `repo/mod.rs` re-export of a
`table`, `tx` or `query` item is a backward pair: add none.

## Lessons (binding)

* No function-local `use` in any file you touch (the structure gate flags every local `use` in a changed file, old ones
  too): hoist them to the file header.
* No intra-doc links of the form `[`x`](Self::x)` or ``[`RepoInstance`]`` to an item that moved or is no longer in
  scope: use plain backticks. `cargo doc` with broken links promoted to errors runs in CI and you cannot run it here;
  re-read every doc comment of every item you touch.
* No inherent `impl` of a lower-layer type in an upper layer. No new `#[allow]`, no visibility widening beyond what a
  moved method needs (list each one in the report: the gate wants it declared).
* Folder limit 7 entries, file limit 1000 lines; a file that would pass it is split in the same change.
* `cfg(test)` / feature / `loom` code is compiled only partly by the local clippy: re-read for unused imports, wrong
  paths and privacy.

## Observable acceptance

`cargo fmt -p shamir-engine -p shamir-db -- --check` and `cargo clippy -p shamir-engine -p shamir-db --all-targets
-- -D warnings` clean; `python -I scripts/ci/check_layout.py` OK; `grep -rw RepoInstance` over the production files under
`tx/` finds nothing but (at most) comments you have rewritten; every public path unchanged. The final report lists every
converted function and field, every method moved onto `RepoCore` in this stage, every call site outside `tx` that you
changed, every widened visibility, the drainer hand-over before and after (file and line), and what must be observed in
CI. CI observes the rust-guard gates (the orchestrator fills the campaign delta from the findings), `layers`, rustdoc,
engine-features, loom-model and the full matrix. Non-goals: the `query/batch` rewrite and ports, crate creation,
behaviour change, version bumps.
