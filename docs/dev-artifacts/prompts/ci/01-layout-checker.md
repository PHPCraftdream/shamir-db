# CI-01 — automatic layout checker (<= 7 entries per folder, <= 1000 lines per file)

## Context

`CLAUDE.md` ("Layout limits") requires at most 7 direct entries per folder
(files plus subfolders, `mod.rs` / `lib.rs` included) and at most 1000
physical lines per source file (`.rs`, `.ts`, `.py`, `.sh`, as `wc -l`
counts them), in source trees only — not lockfiles, generated code, vendored
code, fixtures or docs. Today nothing enforces it. The owner decided
(2026-10-16) that both limits are enforced by an automatic checker, and that
existing violations will be swept into subfolders / split files afterwards.

## Task

1. `scripts/ci/check_layout.py` (Python 3, stdlib only, runnable as
   `python3 -I scripts/ci/check_layout.py`):
   - Walks git-tracked files only (`git ls-files`), so `target/`, untracked
     scratch and ignored paths never count.
   - Scope: folders under `crates/`, `scripts/`, `tests/`, `examples/` and any
     other source root that holds `.rs`/`.ts`/`.py`/`.sh` files; excluded:
     `vendor/`, `docs/`, generated code, fixtures (data files such as
     `.json`, `.bin`, `.txt` test vectors do not count as entries' content
     but a folder's entry count includes every tracked direct child — define
     and document the precise rule in the script header, consistent with
     CLAUDE.md), `node_modules`, lockfiles. Keep the exclusion list in one
     place at the top of the script.
   - Folder rule: number of direct tracked children (files + subfolders)
     <= 7. File rule: physical line count <= 1000 for `.rs`, `.ts`, `.py`,
     `.sh`.
   - Ratchet baseline: `scripts/ci/layout_baseline.txt` (committed, sorted,
     one entry per line: `dir <path> <count>` / `file <path> <lines>`)
     records every CURRENT violation with its current size. The check fails
     when (a) a folder or file not in the baseline violates a limit, or (b) a
     baselined folder/file grew above its recorded size. A baselined entry
     that shrank or is now within the limit prints a hint to lower/remove it
     (`--update-baseline` rewrites the file with current values and only ever
     lowers or removes entries; it never adds new violations).
   - Clear output: one line per violation with the path, the measured value
     and the limit; exit code 1 on any failure, 0 otherwise.
   - Generate the initial baseline with the script itself (the orchestrator
     will re-generate it in CI if your local tree differs; describe the
     command in the final message). Do not hand-edit counts.
2. `scripts/ci/tests/test_check_layout.py` (unittest, like
   `test_affected_crates.py`): fixtures built in a temporary git repo or via
   an injectable file list; cover folder limit, file limit, exclusions,
   baseline pass, baseline growth failure, new violation failure, baseline
   shrink hint, `--update-baseline` never adding entries.
3. CI: run the checker in the FAST tier job and in the FULL tier (a cheap
   step next to the existing `affected_crates.py unit tests` steps in
   `.github/workflows/ci.yml`); the existing unittest discover step already
   picks up the new test file. The checker must run for every PR, including
   docs-only diffs (it is instant).
4. Update the "Layout limits" section of `CLAUDE.md` to name the checker,
   the baseline file and the ratchet rule (keep it short).

Non-goals: moving or splitting any existing file/folder (that is the
follow-up sweep); changing any Rust code.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. Read-only git (`ls-files`, `log`,
  `diff`, `show`) is fine.
- NO `cargo` build/test/check/clippy/nextest. You MAY run the new Python
  script and its unit tests (`python3 -I -m unittest discover -s
  scripts/ci/tests`) — they are instant and touch no Rust build.
- No machine-specific absolute paths in any file you write.
- The new script and test files must themselves respect both limits; the
  `scripts/ci/` folder must stay within 7 entries (count it, including
  `tests/`).

## Final message

The checker's rules (exact scope/exclusions), the baseline size (number of
folder and file violations), the CI wiring with line numbers, and the
unit-test results you ran.
