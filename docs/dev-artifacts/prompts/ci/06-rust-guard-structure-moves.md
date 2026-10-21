# rust-guard `structure`: base-aware rules for split and moved files

## Why

The engine layout sweep splits large files into many new files. The
`structure` gate (`tools/rust-guard/src/structure_cmd/`, wired in the
`source-integrity` job as `structure --repo . --changed-since <merge-base>
--scope <package dir>` for every active campaign in `.rust-guard/stages.toml`)
only recognises a WHOLE-FILE move (`diff.rs::find_moved`: a head file absent
from base that is token-identical, as a multiset of items, to ONE vacated base
file). A file that is the result of a split is "brand-new" to it, so every item
it carries is judged against an EMPTY base: `check_new_file` reports
`VIS_WIDENING` for each `pub(crate)`/`pub` item and `check_allows` reports
`ALLOW_ADDED` for each pre-existing `#[allow(...)]`; and the rule that flags
inline test modules (`INLINE_TEST_MOD`, `rules.rs`) fires on a pre-existing
`#[cfg(loom)] mod loom_model { ... }` that merely moved with its file.
A real run on the first engine stage produced 166 `VIS_WIDENING`, 20
`ALLOW_ADDED` and 1 `INLINE_TEST_MOD` finding — every one of them an item that
exists unchanged in the base tree, in a file that the PR deleted. The delta
file cannot help (the CI step passes no `--delta` to `structure`, and one entry
per item is exactly the paperwork we want to avoid).

## What to build

Make rules 6 (`VIS_WIDENING`, `ALLOW_ADDED`) and the inline-test-module rule
compare a brand-new head file against the whole set of VACATED base files, not
against an empty file:

- VACATED base files = base `.rs` files that no longer exist at head (the same
  list `check_files` already builds as `base_rs`). Index their items with the
  existing `vis_index` machinery into one GLOBAL map `ItemKey -> max rank`
  (key = kind + qualified name, impl members keyed by impl self type, inline-module
  prefix included, exactly as today). Build it lazily, once per run.
- `check_new_file`: an item of a brand-new (not whole-file-moved) head file is a
  `VIS_WIDENING` only if the global vacated index has NO entry with the same
  key and rank >= the head item's rank. An item that exists nowhere in the
  vacated base, or only with a lower rank, is still a finding (a genuinely new
  or widened item).
- `ALLOW_ADDED` for a brand-new head file: an attribute is "new" only if the
  vacated base files contain the same normalised attribute text attached to an
  item with the same key; otherwise unchanged behaviour.
- Inline test module rule: a brand-new head file's inline test module is
  excused only if a vacated base file contained an inline test module with the
  same name AND the same set of `#[test]`/`#[tokio::test]` function names;
  otherwise unchanged.
- Whole-file-move pairing (`find_moved`) and the per-file rules for files present
  in both revisions stay as they are. Nothing that is new, widened or newly
  suppressed may become silent: the new fallback only ever REMOVES findings
  that are backed by an identical item in a deleted base file.
- Duplicates: if several vacated base items share a key, the head item is
  covered if ANY of them has rank >= its rank (document this).

## Tests

Fixtures + tests in `tools/rust-guard/tests/` following the existing structure
tests: (1) a base file with `pub(crate) fn a`, `#[allow(dead_code)] fn b` and an
inline `#[cfg(loom)] mod loom_model` split at head into two new files that
carry the same items -> exit 0; (2) the same split where one item became
`pub` -> `VIS_WIDENING` for exactly that item; (3) a split where an
`#[allow(...)]` is NEW (not in base) -> `ALLOW_ADDED`; (4) a split where the
inline test module gained a test fn -> still flagged; (5) the item exists in a
base file that STILL exists at head (copy, not move) -> still flagged as
before. Update `tools/rust-guard/README.md` (rule semantics, "split files are
compared with the vacated base files"). Tests run in CI only.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
  or any git command that mutates the working tree or index (including
  `git add` / `git commit`). Only edit files; the orchestrator commits.
- Compile / build / test policy: the policy text at the top of this run
  (verbatim) applies. `tools/rust-guard` is workspace-excluded: allowed and
  REQUIRED until clean are
  `cargo check --manifest-path tools/rust-guard/Cargo.toml --all-targets`,
  `cargo clippy --manifest-path tools/rust-guard/Cargo.toml --all-targets -- -D warnings`
  and `cargo fmt --manifest-path tools/rust-guard/Cargo.toml`
  (`CARGO_BUILD_JOBS=2`). No `cargo test`/`build`/`run`.
- Do the editing yourself; start at most ONE child agent at a time, read-only
  verification, 25-minute budget, never wait longer (waiting counts against the
  150-minute session limit). Stop editing at 100 minutes; the final message is
  mandatory.
- No `xargs`; no machine-specific paths; imports at the top; `mod.rs` files
  re-export only; keep every file under 1000 lines (`structure_cmd/diff.rs` is
  already 640 lines: put the new base-index code in a NEW sibling file, e.g.
  `structure_cmd/vacated.rs`, and keep every folder under 8 entries — check
  `ls structure_cmd` first and split into a subfolder if needed).
- Keep the change minimal and self-contained; do not refactor unrelated code.

## Final message

Files changed, how each of the four behaviours above is implemented
(file:line), the fixture list, real exit codes of check/clippy/fmt, and
anything you could not verify.
