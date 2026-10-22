# Engine layout sweep — stage D3 (repository tests)

## Context

`scripts/ci/check_layout.py` enforces at most 7 direct entries per folder and
at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh` file; existing
violations are grandfathered in `scripts/ci/layout_baseline.txt` and the owner
decided to sweep them until the baseline is empty. `shamir-engine` is swept in
stages; the query, validator, benches, integration-test and table stages are
merged, as are the transaction-preparation (B1) and drain (C1) stages; the
remaining baseline is the rest of the transaction/repository "spine". This is
stage D3 (`repo/tests` regroup). It is a PURE STRUCTURAL REFACTOR: behaviour, public API paths,
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

- `crates/shamir-engine/src/repo/tests/` (21 direct entries): regroup into at most 7 direct entries by topic, WITHOUT moving anything outside this folder. Use the manifest's topical names (`05b-4-repository.md`, rows `repo/tests/*`, destinations `repo/journal/tests/{journal,core,lifecycle,identity}/...`) but place the folders directly under `repo/tests/`: `core/`, `journal/`, `lifecycle/`, `identity/` (and a fifth only if a file fits none of them, e.g. `changelog_store_tests.rs`, `prepare_seed_tests.rs`: put them where they belong topically). `repo/tests/mod.rs` stays a manifest (`mod` declarations only, wired from `repo/mod.rs` as today) plus a `mod.rs` per new folder. Test function names stay identical; test file contents are unchanged except `use super::...` / `use crate::...` paths that the extra nesting level breaks (`super::` now means the group folder).
- Shared helpers that several moved test files import from each other (grep `super::` / `crate::repo::tests::`) keep a stable path: put them in a `support` module (NOT named `fixtures`).
- NOT in scope: every non-test file under `src/repo/` (the parallel stage D1 moves `repo/repo_instance.rs`), `src/tx/` (stage E1t), `src/table/`, everything else. The folder `repo/tests/` must end with at most 7 direct entries and each new folder with at most 7 as well.

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

Layout ratchet reminder: the folders `crates/shamir-engine/src/repo` (19 direct
entries) and `crates/shamir-engine/src/repo/tests` (21) and `crates/shamir-engine/src/tx` (21) /
`src/tx/tests` (60) are over the limit and baselined. A change must NOT grow any of them.
Turning `foo.rs` into a directory module keeps the entry count; adding a sibling
`foo.rs` PLUS a `foo/` folder, or a new top-level folder, grows it and is forbidden unless
the same change removes at least as many direct entries from that folder.

Stage D3 (repository tests) notes:

- Two other stages run in parallel in other worktrees on different files. Do not touch their files (listed under Scope as NOT in scope). Shared manifests (`src/repo/mod.rs`, `src/repo/tests/mod.rs`, `src/tx/tests/mod.rs`, `src/tx/mod.rs`) change only on the lines of your own modules; the orchestrator resolves textual merge conflicts there.
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
