# Engine layout sweep — stage Q2 (validator, integration test crash_recovery, benches)

## Context

`scripts/ci/check_layout.py` enforces at most 7 direct entries per folder and
at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh` file; existing
violations are grandfathered in `scripts/ci/layout_baseline.txt` and the owner
decided to sweep them until the baseline is empty. Every other crate is done;
`shamir-engine` is the last big one and is swept in stages. This is stage
Q2 (validator, integration test crash_recovery, benches). It is a PURE STRUCTURAL REFACTOR: behaviour, public API paths, error
behaviour, test semantics and test names stay identical.

Read first (committed on master):

- `docs/dev-artifacts/prompts/ci/05a-engine-sweep-plan.md` — adjudicated plan: privacy findings, risks, what stays manual.
- `docs/dev-artifacts/prompts/ci/05b-engine-manifest.md` — index of the routing manifest, and the manifest part for this stage: `docs/dev-artifacts/prompts/ci/05b-1-query.md` (stage Q).

The manifest is a ROUTING GUIDE, not an authorization: it fixes source ->
destination routing, but you must re-derive visibility, privacy and sizes from
the code. Where a destination in the manifest is wrong (a zero-line estimate,
a privacy audience that is not an ancestor, a folder that would exceed 7
entries) deviate deliberately and list every deviation (source, manifest
destination, actual destination, reason) in your final message. The
authorities are the compiler gates below and CI.

## Scope

- `crates/shamir-engine/src/validator/` (18 entries), `validator/schema` (11), `validator/tests` (13).
- `crates/shamir-engine/tests/crash_recovery.rs` (1651 lines; it mounts `tests/clean_horizon/crash_matrix.rs` via `#[path]`-style relative mounts — read it first; the module reachability gate treats files below `tests/` as roots only for the immediate-child form).
- `crates/shamir-engine/benches/` (39 entries) and `benches/tx_concurrent.rs` (1013 lines). Bench targets are listed in `crates/shamir-engine/Cargo.toml`; when a bench file moves into a subfolder add/adjust the explicit `[[bench]] path = "..."` entry (the manifest part gives the destination of each bench). Keep every `required-features`.
- NOT in scope: `query/**` (other sessions), `table/**`, `tx/**`, `repo/**`.

Every baseline entry under these paths must be resolved (folder at most 7
direct entries, file at most 1000 lines) and then removed from the baseline
with `python -I scripts/ci/check_layout.py --update-baseline` (it only lowers
or removes entries; run it after your files are tracked is impossible for you
because you cannot `git add` — the orchestrator runs the baseline update; you
only make sure by hand with `ls -A <dir> | wc -l` and `wc -l` that every
folder and file in scope meets the limits). Touch nothing outside the scope
except the minimal reference updates a move requires (`mod` declarations,
imports in other modules/crates, `#[path]`, `include!`, docs that name a
moved file). Do NOT edit `scripts/ci/layout_baseline.txt`,
`.rust-guard/**`, `.github/**` or any file of another stage.

Stage Q2 notes:

- You own `crates/shamir-engine/Cargo.toml` for this stage (bench paths only; do not touch dependencies or features). Do not edit `src/lib.rs` except if a `validator` module path really changes (it should not: keep re-exports).
- Benches are not compiled by `cargo check --all-targets` unless their `required-features` are enabled: run `cargo check -p shamir-engine --all-targets --features test-util` too (and `--features fjall,test-util` if the manifest declares such a feature) so moved benches are type-checked; benches are NEVER run.
- Integration tests under `crates/shamir-engine/tests/`: a `tests/<name>.rs` file is a test target; its helper modules live in subfolders mounted by that file. Keep every test target name unchanged (CI selects them by binary name).

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
