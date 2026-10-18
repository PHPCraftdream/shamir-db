# CI-02 — layout sweep: shamir-query-types + shamir-query-builder (+ shamir-query-builder-macros)

## Context

`scripts/ci/check_layout.py` (merged in #187) enforces at most 7 direct
entries per folder and at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh`
file; current violations are grandfathered in `scripts/ci/layout_baseline.txt`.
The owner decided (2026-10-16) to sweep them until the baseline is empty.
This PR sweeps exactly the scope below. It is a PURE STRUCTURAL REFACTOR:
behaviour, public API paths, error behaviour, test semantics and test names
stay identical.

## Scope

- `crates/shamir-query-types/`
- `crates/shamir-query-builder/`
- `crates/shamir-query-builder-macros/`

Every baseline entry under these paths must be resolved (folder at most 7
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
  (prefer this over editing callers). For TypeScript, keep the package's
  public exports identical (barrel `index.ts` re-exports) and update relative
  imports.
- Files: split along their seams (separate `impl` blocks, helper groups,
  test topics), each new file keeping "one file = one primary export";
  `impl` blocks of one type may live in several files of the same module.
  Do not change signatures or visibility beyond what the move needs
  (`pub(super)` → `pub(crate)` only if a split genuinely requires it).
- A crate root folder (`crates/<crate>/` itself, holding Cargo.toml,
  package.json, configs) may not be reducible below 8 without breaking
  tooling; if so leave that single entry in the baseline and explain why in
  the final message. Everything else must reach the limit.
- Tests: moved test modules keep their test function names; nextest filters
  by module path change, which is acceptable.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. Moving a file = create the new file
  with your file tool and delete the old one with your file tool (never
  `git mv`).
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` / `npm` /
  `tsc` or any other compile or test command, by you or any child agent
  (repeat this rule in every child brief). Allowed: `cargo fmt -p <crate>`
  as the last step, and `python -I scripts/ci/check_layout.py` (instant, no
  build). CI is the gate. Clippy runs with `-D warnings` (unused imports
  after moves, `dead_code` — gate test-only items with `#[cfg(test)]`).
- Never silence a lint with `allow`.
- Imports at the top of each file; `mod.rs` files only re-export.
- No machine-specific absolute paths in any file you write.
- Before finishing, one verifier child agent (same no-compile rule) reads in
  full every new/changed file and checks: every moved item is still
  reachable at its old public path, every `mod` declaration matches a file,
  no item was dropped or duplicated (compare item lists before/after with
  read-only `git show origin/master:<path>`), imports resolve, test modules
  are all wired into their `tests/mod.rs`.

## Final message

The before/after entry count of every folder and line count of every file in
scope, the checker's final output, and any baseline entry intentionally left
(with the reason).
