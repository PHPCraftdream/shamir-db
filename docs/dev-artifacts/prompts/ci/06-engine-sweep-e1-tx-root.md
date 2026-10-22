# Engine layout sweep — stage E1 (transaction root regroup)

## Context

`scripts/ci/check_layout.py` enforces at most 7 direct entries per folder and
at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh` file; existing
violations are grandfathered in `scripts/ci/layout_baseline.txt` and the owner
decided to sweep them until the baseline is empty. `shamir-engine` is swept in
stages; the query, validator, benches, integration-test and table stages are
merged, as are the transaction-preparation (B1) and drain (C1) stages; the
remaining baseline is the rest of the transaction/repository "spine". This is
stage E1 (`tx/` root: commit/preparation/drain/recovery/replication). It is a PURE STRUCTURAL REFACTOR: behaviour, public API paths,
lock/guard lifetimes, drop order, cancellation and unwind ownership, durability
order, error behaviour, test semantics and test names stay identical.

Read first (committed on master):

- `docs/dev-artifacts/prompts/ci/05a-engine-sweep-plan.md` — adjudicated plan: privacy findings, risks, what stays manual (read the transaction sections and the "Manual review per extraction" list: lock/guard lifetime, drop order, cancellation/unwind ownership, durability/publication/checkpoint/unlink order, identity preservation).
- `docs/dev-artifacts/prompts/ci/05b-engine-manifest.md` — index of the routing manifest, and the manifest part for this stage: `docs/dev-artifacts/prompts/ci/05b-3-transactions.md`.

The manifest is a ROUTING GUIDE, not an authorization, and it was written before
the table/query sweeps and before #189: it fixes source -> destination routing,
but you must re-derive visibility, privacy and sizes from the code AS IT IS NOW.
Where a destination in the manifest is wrong or would break a rule below (a
zero-line estimate, a privacy audience that is not an ancestor, a folder that
would exceed 7 entries, a NEW direct entry in a folder that is already over the
limit), deviate deliberately and list every deviation (source, manifest
destination, actual destination, reason) in your final message. The authorities
are the compiler gates below and CI.

## Scope

- `crates/shamir-engine/src/tx/` has 21 direct entries; it must end with exactly these seven: `mod.rs`, `tests/` (already regrouped, do not touch its inside), `commit/`, `preparation/`, `drain/`, `recovery/`, `replication/`. Route by the manifest `docs/dev-artifacts/prompts/ci/05b-3-transactions.md` (rows with source under `tx/`, EXCEPT `tx/tests/*`), dropping its `owned/` prefix because the groups sit directly under `tx/`. The manifest predates stages B1/C1/B2/C2, so re-derive the current layout of each moved module from the code; the intended grouping is:
  - `commit/` (existing dir module from stage B2: `mod.rs`, `entry.rs`, `legacy_and_lockfree.rs`) absorbs `ack/` -> `commit/acknowledgement/`, `commit_driver.rs`, `commit_tail_barrier.rs`, `tx_outcome.rs` -> `commit/pipeline/driver/`, `commit_phases.rs`, `finalize.rs`, `materialize.rs`, `publish_journaled.rs` -> `commit/pipeline/phase/`; the B2 files (`entry.rs`, `legacy_and_lockfree.rs`) move under `commit/pipeline/phase/commit/` together with their `mod.rs` (the logical path `crate::tx::commit` stays).
  - `preparation/`: `pre_commit/` (whole folder, stage B1 output, do not reorganize its inside) and `predicate_range.rs`.
  - `drain/`: `drainer/` (whole folder, stage C1 output) and `orphan_repair/` (-> `drain/drainer/orphan_repair/` per the manifest only if that does not push `drain/drainer/` over 7 entries; otherwise `drain/orphan_repair/`).
  - `recovery/`: `clean_horizon/`, `cold_reconcile/`, `recovery.rs`.
  - `replication/`: `apply_replicated.rs`, `replicated_event.rs`, `follower_durability/` (whole folder incl. its `receipt/` and tests).
- Every group folder, every sub-folder and every file in them respects at most 7 direct entries and at most 1000 lines (`ls -A`, `wc -l`). A new `mod.rs` per group is a manifest of `mod` declarations and re-exports only; a module that already has a `mod.rs` keeps it, with `#[path]`/`mod` lines adjusted.
- `tx/mod.rs` stays the manifest: `pub(crate) mod` / `mod` declarations of the 6 groups and re-exports that keep EVERY old path alive with the exact same audience (`crate::tx::{pre_commit,drainer,orphan_repair,clean_horizon,cold_reconcile,follower_durability,commit,ack,recovery,apply_replicated,replicated_event,commit_driver,commit_phases,commit_tail_barrier,finalize,materialize,publish_journaled,predicate_range,tx_outcome}` and all item re-exports that exist today; grep `shamir_engine::tx::` in `crates/**` and `crate::tx::` in the engine). Prefer `pub(crate) use <group>::<module>;` style re-exports over editing call sites; rewrite internal paths only where the old path cannot be kept. The tests under `tx/tests/**` and the test modules inside the moved folders reach the code through those preserved paths and must not need edits except `#[path]`/`super::` strings that the extra nesting level breaks.
- `.rust-guard/owned_impl.toml` asserts that every inherent `impl RepoInstance` lives in `crate::repo::instance::repo_instance` or a descendant, and the guard fires on every CHANGED file that still holds such an impl outside it. The file `follower_durability/receipt/resume/worker.rs` holds `impl RepoInstance { receipt_worker_installed, submit_receipt_job, ... }` at its end: move that `impl` block (methods byte-for-byte, same signatures, doc comments, `#[cfg]`, visibility audience) into a new child file of `crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs` (`repo/instance/repo_instance/transactions/receipt_worker_impl.rs`, mounted with a `mod receipt_worker_impl;` line in `transactions.rs`; if `transactions.rs` has no `transactions/` folder yet, create it; keep the part under 1000 lines) and leave the free functions of `worker.rs` where they are. Items of `worker.rs` that the moved methods use become `pub(in crate::tx)`-style narrowest audience; list each widening with file:line. Stage D2 runs in parallel in `src/repo/` and only adds/edits `mod` lines in `repo_instance.rs` and the parts: touch NOTHING else under `src/repo/`.
- Behaviour: every moved item keeps its body, signature, `#[cfg]`/feature gate and visibility audience. A `pub(super)` item that moves one level deeper silently narrows its audience: re-derive each as the exact old audience (`pub(in crate::tx)`, `pub(in crate::tx::commit)`, ...) and list it. Cross-module private fields keep working by keeping type-defining files together with their impl parts as descendants.
- NOT in scope: `tx/tests/**` insides, everything under `src/repo/` (except the one `receipt_worker_impl.rs` step above), `src/table/`, `src/db_instance/`, other crates.

Every baseline entry under these paths must be resolved (folder at most 7
direct entries, file at most 1000 lines). Do NOT edit
`scripts/ci/layout_baseline.txt` yourself: you cannot `git add`, the
orchestrator runs `python -I scripts/ci/check_layout.py --update-baseline` after
your files are tracked; you only make sure by hand with `ls -A <dir> | wc -l` and
`wc -l` that every folder and file in scope meets the limits. Touch nothing
outside the scope except the minimal reference updates a move requires (`mod`
declarations, imports in other modules/crates, `#[path]`, `include!`, docs that
name a moved file). Do NOT edit `.rust-guard/**`, `.github/**`,
`crates/shamir-engine/Cargo.toml` or any file of another stage.

Layout ratchet reminder: the folder `crates/shamir-engine/src/tx` has 21 direct
entries (over the limit, baselined) and must END with at most 7. A change must NOT
grow any other over-limit folder (`src/repo` runs through stage D2 in parallel: do not
touch it). Turning `foo.rs` into a directory module keeps the entry count; adding a
sibling `foo.rs` PLUS a `foo/` folder grows it and is forbidden unless the same change
removes at least as many direct entries from that folder.

Stage E1 (transaction root regroup) notes:

- Stage D2 (`src/repo/` root regroup) runs in parallel in another worktree. Do not touch its files (listed under Scope as NOT in scope). Shared manifests (`src/repo/mod.rs`, `src/repo/tests/mod.rs`, `src/tx/tests/mod.rs`, `src/tx/mod.rs`) change only on the lines of your own modules; the orchestrator resolves textual merge conflicts there.
- Every `crate::repo::<module>` / `crate::tx::<module>` path used anywhere in the workspace (grep `shamir_engine::repo::`, `shamir_engine::tx::` in all crates and `crate::repo::`, `crate::tx::` in the engine) must keep resolving with the same visibility audience; keep the old module path and the old item paths through `pub use` / `pub(in ...) use` re-exports in the new `mod.rs`. A `pub(super)` item inside a file that moves one level deeper would silently narrow its audience: re-derive each such visibility as the exact old audience (`pub(in crate::repo)` etc.) and list it.
- Test-seam hooks and statics consumed by `shamir-db` / `shamir-server` tests under `test-util` keep their public paths and their cfg/feature gating exactly.
- Guard types, lock guards and the order in which they are acquired, held across `.await` and dropped must be byte-for-byte the same code in the same order.
- `#[path = "..."]` on a `mod x;` resolves relative to the directory of the file that contains it (for a non-`mod.rs` file: its own directory; for `mod.rs`: the same directory, so the string changes by one `../` when `foo.rs` becomes `foo/mod.rs` or moves one level deeper). Re-check every `#[path]` and every `include!`/`include_str!` in the moved files and in the files that mount them.
- A module directory must not contain a file with the same name as the directory (clippy `module_inception`) and must not be named `fixtures` (rust-guard and the layout checker skip such directories).
- Do not rename, reorder statements, or "improve" anything. Do not delete dead code.

## How to move and split

- Folders: group entries into topical subfolders (tests: `tests/<topic>/…`),
  each subfolder with its own `mod.rs` manifest (declarations and re-exports
  only). Keep every public Rust path stable with `pub use` re-exports from the
  original module so no caller in any crate changes (prefer this over editing
  callers). Crates that depend on shamir-engine must keep compiling unchanged.
- Files: split along real seams (separate `impl` blocks, helper groups, test
  topics); each new file keeps "one file = one primary export"; `impl` blocks
  of one type may live in several files of the same module tree.
- Bodies of moved items stay byte-identical. The only permitted edits inside a
  moved item are module-path fixes (`super::`/`crate::` after a move). No
  reformatting beyond `cargo fmt -p shamir-engine`, no renames, no signature
  changes, no new lint suppressions, no new `pub` beyond what a move strictly
  needs (`pub(super)` -> `pub(crate)` only where a split genuinely requires
  it; list each such change with file:line in the final message).
- Privacy: an `impl` moved away from its type's module loses access to
  private fields and methods. Keep such `impl` blocks in the type's own module
  or in a DESCENDANT of it (child modules see the parent's private items);
  never widen field visibility to make a sibling compile.
- Tests: moved test modules keep every `#[test]`/`#[tokio::test]` function
  name and every `#[cfg(...)]`/`#[ignore]` attribute; every test file is wired
  through its `tests/mod.rs` (`pub mod x;` lines only) and the parent
  `#[cfg(test)] mod tests;`. Never inline `#[cfg(test)] mod tests { }` into an
  implementation file. A test file above 1000 lines is split by topic
  (shared fixtures go into a `helpers`/`support` module of the same test tree).
- Imports at the top of each file; `mod.rs` files contain declarations and
  re-exports only.

## Lessons from earlier sweeps (CI found 20+ errors of these classes; check each yourself)

- a new module file must be declared with `mod x;` exactly once in the right parent;
- `super::` / `crate::` paths change when a file moves one level deeper;
- an `impl` moved into a sibling module loses access to private items (see Privacy);
- imports left unused after code moved out must be removed; test-only imports need `#[cfg(test)]`;
- a module and a re-exported item with the same name collide; a private item must not shadow a glob re-export;
- `#[path = "..."]` mounts and `include!` resolve relative to the file that contains them — re-check every one after a move (the engine has logical mounts that differ from the filesystem layout, e.g. in `repo/repo_instance.rs`, `tx/apply_replicated.rs`, `tx/drainer.rs`);
- Linux-/Windows-/feature-gated code is not compiled on this machine: for every moved item with `#[cfg(...)]` check by reading that its gate and its imports moved with it;
- `[[bench]]`/`[[test]]` targets in `crates/shamir-engine/Cargo.toml` use explicit `path` entries when a file moves; auto-discovered targets break silently;
- a closing brace lost at the end of a split file (check balance of every file you cut).

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator
  commits. Moving a file = create the new file with your file tool and delete
  the old one with your file tool (never `git mv`).
- Compile / build / test policy: see the policy text at the top of this run
  (verbatim). Allowed locally and REQUIRED until clean:
  `cargo check -p shamir-engine --all-targets`,
  `cargo clippy -p shamir-engine --all-targets -- -D warnings`,
  `cargo fmt -p shamir-engine`, and the same `cargo check --all-targets` for
  `-p shamir-db` and `-p shamir-server` (they consume the engine API). Also run
  `cargo check -p shamir-engine --no-default-features --lib` once at the end.
  One cargo process at a time, `CARGO_BUILD_JOBS=4`.
- Never use the shell builtin `xargs` (crashes the rush session). Capture exit
  codes without pipes. Logs under `.scratch/`.
- No machine-specific absolute paths in any file.
- Before finishing, one verifier child agent (the whole policy pasted
  verbatim) reads in full every new/changed file and checks: every moved item
  is still reachable at its old public path, every `mod` declaration matches a
  file, no item was dropped or duplicated (compare item lists before/after
  with read-only `git show origin/master:<path>`), imports resolve, test
  modules are all wired into their `tests/mod.rs`, braces balance, function
  bodies are unchanged except for path fixes.

## Final message

1. Before/after entry count of every folder and line count of every file in scope.
2. The real exit codes of check/clippy/fmt for each crate listed above.
3. Test conservation table: number of `#[test]` + `#[tokio::test]` functions in scope before (`git show origin/master:<path>`) and after, per old file.
4. Deviations from the manifest (see Context), every visibility widening, every `Cargo.toml` change.
5. Anything you could not verify.
