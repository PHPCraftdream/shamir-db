# Engine layout sweep — stage F (engine src root, #[path] removal, docs)

## Context

`scripts/ci/check_layout.py` enforces at most 7 direct entries per folder and
at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh` file; existing
violations are grandfathered in `scripts/ci/layout_baseline.txt` and the owner
decided to sweep them until the baseline is empty. `shamir-engine` is swept in
stages; the query, validator, benches, integration-test and table stages are
merged, as are the transaction-preparation (B1) and drain (C1) stages; the
remaining baseline is the rest of the transaction/repository "spine". This is
stage F (`src/` root regroup, `#[path]` removal, docs). It is a PURE STRUCTURAL REFACTOR: behaviour, public API paths,
lock/guard lifetimes, drop order, cancellation and unwind ownership, durability
order, error behaviour, test semantics and test names stay identical.

Read first (committed on master):

- `docs/dev-artifacts/prompts/ci/05a-engine-sweep-plan.md` — adjudicated plan: privacy findings, risks, what stays manual (read the transaction sections and the "Manual review per extraction" list: lock/guard lifetime, drop order, cancellation/unwind ownership, durability/publication/checkpoint/unlink order, identity preservation).
- `docs/dev-artifacts/prompts/ci/05b-engine-manifest.md` — index of the routing manifest, and the manifest part for this stage: `docs/dev-artifacts/prompts/ci/05b-4-repository.md`.

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

- `crates/shamir-engine/src/` has 12 direct entries (`LAYERS.md`, `README.md`, `db_instance/`, `index/`, `lib.rs`, `meta/`, `migration/`, `query/`, `repo/`, `table/`, `tx/`, `validator/`). It must end with exactly these seven: `lib.rs`, `db_instance/`, `query/`, `repo/`, `table/`, `tx/`, `validator/`. Routing (manifest `docs/dev-artifacts/prompts/ci/05b-4-repository.md`, rows with stage F, plus one deliberate deviation):
  - `src/LAYERS.md` -> `crates/shamir-engine/docs/LAYERS.md`; `src/README.md` -> `crates/shamir-engine/docs/README.md`; `src/index/README.md` -> `crates/shamir-engine/docs/index_README.md`; `src/index/index_types.md` -> `crates/shamir-engine/docs/index_index_types.md` (the `docs/` folder already exists from stage D2 with `repo_README.md`; it ends with at most 7 entries; fix relative links inside the moved texts and any `include_str!`/doc reference to them).
  - `src/index/mod.rs` is a pure re-export facade of `shamir_index::base_index::*` modules: it becomes an inline `pub mod index { pub use shamir_index::base_index::index_definition; ... }` in `lib.rs` (the file `index/mod.rs` and the folder `index/` disappear; every `crate::index::...` and `shamir_engine::index::...` path keeps resolving with the same items and visibility).
  - `src/migration/` -> `src/db_instance/migration/` (coordinator, shadow_key, shadow_log, tests; manifest rows `migration/* -> db_instance/migration/*`). Keep the old crate path alive: `lib.rs` gets `pub use db_instance::migration;` so `crate::migration::...` and `shamir_engine::migration::...` (grep all crates) keep working; `db_instance/mod.rs` declares `pub mod migration;` (re-exports/declarations only). `db_instance/` must end with at most 7 entries.
  - `src/meta/` -> `src/repo/meta/` (deviation from the manifest, which leaves `meta/` at the root: with the manifest routing `src/` would still have 8 entries; `meta` holds repository metadata records: repo_identity, recovery_marker, envelope, namespace, repl bookmarks/receipts, so it belongs under `repo/`). Keep `crate::meta::...` / `shamir_engine::meta::...` alive with `pub use repo::meta;` in `lib.rs`; `repo/mod.rs` gets `pub mod meta;`; `repo/` must end with at most 7 entries (it has 5 now). Check `meta/repl/mod.rs` and the `meta/tests` folder: any `super::`/`crate::meta::` path and any `#[path]` keeps working.
  - Report the final `ls -A` of `src/`, `docs/`, `db_instance/`, `repo/`.
- **`#[path]` removal.** `grep -rn '#\[path' crates/shamir-engine/src` lists the mounts left by the sweep (about 20 outside doc comments). Remove every `#[path]` that can be expressed as an ordinary child module declaration (`mod x;` resolving to `x.rs` or `x/mod.rs` next to the declaring file, nested `mod` declarations in the intermediate `mod.rs` files for deeper trees) WITHOUT changing the logical module path of any item, any visibility audience or any `cfg`: candidates are `table/table_manager.rs` and `table/mod.rs` (the `table_manager/...` mounts), `tx/drainer/state.rs`, `tx/drainer/mod.rs` (`orphan_repair`), `tx/apply_replicated.rs` (`follower_durability`), `tx/cold_reconcile/**`, `tx/follower_durability/**`, `tx/orphan_repair/mod.rs`, `meta/repl/mod.rs` (`#[path = "tests/mod.rs"] mod tests;` is just `mod tests;` when the file is a `mod.rs`). Mounts that cannot become ordinary children without changing a logical path or a private-field audience stay (the cross-tree mounts `tx/commit/**/entry.rs` -> `repo/journal/backend/durability_policy/**/commit_policy.rs`, `repo/tests/mod.rs`, `tx/tests/mod.rs` old test paths): list each remaining `#[path]` in the final message with the reason. `table_manager` impl blocks that are mounted as CHILDREN of `table_manager` so they keep private-field access stay children (a non-`mod.rs` type file with `mod x;` children owns descendants that see its private fields): convert the attribute into nested declarations, never into siblings. Update the stale doc comments that talk about `#[path]` wiring.
- **Docs.** Update every in-repo doc that names a moved path or the old layout: `crates/shamir-engine/docs/*` (moved files), `docs/` pages and `CLAUDE.md`/`AGENTS.md` sections that describe `shamir-engine/src` layout or name `src/README.md`, `src/LAYERS.md`, `src/index/*`, `src/migration`, `src/meta` (grep the repo; do not rewrite unrelated text), and `docs/dev-artifacts/prompts/ci/05b-*.md` is left untouched (it is a historical routing manifest).
- NOT in scope: anything under `src/query/`, `src/validator/`, `src/table/` other than the `#[path]` conversion and doc-comment updates named above, the insides of `tx/` and `repo/` groups beyond `#[path]` conversion and the `meta` mount, tests' content, other crates (except import path fixes that a move strictly requires; prefer preserved re-exports so none are needed).

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

Layout ratchet reminder: the only layout baseline entry left for this crate is
`crates/shamir-engine/src` (12 direct entries). This stage must END with at most 7 and
must remove that entry from `scripts/ci/layout_baseline.txt` (the orchestrator runs
`python -I scripts/ci/check_layout.py --update-baseline` after your files are tracked; you
verify by hand with `ls -A`). No other folder may grow past 7 entries or 1000 lines.

Stage F (engine src root, #[path] removal, docs) notes:

- No other engine stage runs in parallel. All earlier stages (B1, C1, B2, C2, D1, D2, D3, E1, E1t) are merged: `repo/` and `tx/` are already regrouped.
- Every `crate::<module>` / `shamir_engine::<module>` path used anywhere in the workspace (grep in all crates, benches, examples and tests) must keep resolving with the same audience: keep the old module names through `pub use` / `pub(crate) use` re-exports in `lib.rs`. Never leave an unused re-export that trips `unused_imports`: use the existing `#[allow(unused_imports)] // Compatibility paths ...` precedent with a one-line reason only when a path has no caller today.
- `#[path = "..."]` resolves relative to the directory of the file that contains it (for a non-`mod.rs` file: its own directory; for `mod.rs`: the same directory). When a file moves one level deeper, re-check every `#[path]` and every `include!`/`include_str!` inside it and in files that mount it.
- A module directory must not contain a file with the same name as the directory (clippy `module_inception`) and must not be named `fixtures`.
- Subprocess/child tests that re-exec the test binary by logical test path (grep `child_entrypoint`, `CHILD_TEST`, `--exact` in `src/**/tests`) hard-code module paths: after moving `meta` and `migration`, re-derive every such string.
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
