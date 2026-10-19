# CI-02 — layout sweep: shamir-client

## Context

`scripts/ci/check_layout.py` (merged in #187) enforces at most 7 direct
entries per folder and at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh`
file; current violations are grandfathered in `scripts/ci/layout_baseline.txt`.
The owner decided (2026-10-16) to sweep them until the baseline is empty.
Earlier sweeps (query crates #190, shamir-index #193, client-ts #192) are
merged; this PR sweeps exactly the scope below. It is a PURE STRUCTURAL
REFACTOR: behaviour, public API paths, error behaviour, test semantics and
test names stay identical.

## Scope

- `crates/shamir-client/`

Every baseline entry under this path must be resolved (folder at most 7
direct entries, file at most 1000 lines), then removed from the baseline with
`python -I scripts/ci/check_layout.py --update-baseline` (it only lowers or
removes entries) and the checker must exit 0. Touch nothing outside the
scope except the baseline file and, where a moved module's path is referenced
(`#[path]`, `include!`, `mod` declarations, imports in other crates, docs that
name the file), the minimal reference update.

## How to split

- Folders: group entries into topical subfolders (for tests:
  `tests/<topic>/…`), each subfolder with its own `mod.rs` manifest
  (re-exports only). Keep every public Rust path stable with `pub use`
  re-exports from the original module so no caller in any crate changes
  (prefer this over editing callers).
- Files: split along their seams (separate `impl` blocks, helper groups,
  test topics), each new file keeping "one file = one primary export";
  `impl` blocks of one type may live in several files of the same module.
  Do not change signatures or visibility beyond what the move needs
  (`pub(super)` -> `pub(crate)` only if a split genuinely requires it).
- A crate root folder (`crates/<crate>/` itself, holding Cargo.toml, build
  scripts, configs) may not be reducible below 8 without breaking tooling;
  if so leave that single entry in the baseline and explain why in the final
  message. Everything else must reach the limit.
- Tests: moved test modules keep their test function names; nextest filters
  by module path change, which is acceptable. Never lose a test: every moved
  test module stays wired in through its `tests/mod.rs` and the parent
  `#[cfg(test)] mod tests;`.

## Lessons from the earlier sweeps (CI found 20+ errors of these classes — check each yourself with `cargo check` / `clippy`)

- a new module file must be declared with `mod x;` exactly once in the right parent;
- `super::` / `crate::` paths change when a file moves one level deeper;
- an `impl` moved into a sibling/child module loses access to private fields and methods of its type: keep it in a submodule of the type's own module, or widen visibility only as far as needed;
- imports left unused after code moved out must be removed; test-only imports need `#[cfg(test)]`;
- a module and a re-exported item with the same name collide; a private item must not shadow a glob re-export;
- `#[path = "..."]` mounts and `include!` use paths relative to the file that contains them — re-check every one after a move;
- a closing brace lost at the end of a split file (check balance of every file you cut).

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator
  commits. Moving a file = create the new file with your file tool and
  delete the old one with your file tool (never `git mv`).
- Compile / build / test policy: no `cargo test` / `build` / `nextest` /
  `./scripts/test.sh` / `npm` / `tsc`, by you or any child agent. Allowed
  locally, scoped: `cargo check -p <crate> --all-targets`,
  `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo fmt -p <crate>`, `python -I scripts/ci/check_layout.py`
  (also for every dependent crate if a public path could change). CI is the
  gate for tests.
- Never use the shell builtin `xargs` (crashes the rush session). Capture exit
  codes without pipes. Logs under `.scratch/`. One cargo process at a time,
  `CARGO_BUILD_JOBS=4`.
- Never silence a lint with `allow`. Imports at the top of each file;
  `mod.rs` files only re-export. No machine-specific absolute paths.
- Before finishing, one verifier child agent (the whole policy pasted
  verbatim) reads in full every new/changed file and checks: every moved item
  is still reachable at its old public path, every `mod` declaration matches a
  file, no item was dropped or duplicated (compare item lists before/after
  with read-only `git show origin/master:<path>`), imports resolve, test
  modules are all wired into their `tests/mod.rs`, braces balance.

## Final message

The before/after entry count of every folder and line count of every file in
scope, the checker's final output, the real exit codes of check/clippy/fmt,
and any baseline entry intentionally left (with the reason).
