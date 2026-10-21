# rust-guard: `--suggest-delta` for the conservation and discovery gates

## Why

The engine layout sweep moves and splits hundreds of files. The
`conservation` gate (`tools/rust-guard/src/conservation_cmd/`) pairs a moved
item (pass 2 in `matching.rs`: same kind + name + enclosing + signature) only
when a `[[moved]]` entry names BOTH sides (`path`, `item`, `to_path`,
`to_item`); every undeclared pair falls apart into `ITEM_LOST` +
`ITEM_ADDED`, and a moved test becomes `TEST_LOST`. The `discovery` gate
(`discovery_cmd/`) has the same shape for compiled test identities
(binary id + test name). Writing thousands of identical-looking entries by
hand is error-prone and gives a reviewer nothing; the only information in
such an entry is the pairing, which the tool already computes.

## What to build

A new optional flag `--suggest-delta <path>` on `rust-guard conservation` and
on `rust-guard discovery`:

- It does not change the normal behaviour: without the flag nothing changes
  (same findings, same exit code, same output).
- With the flag, the comparison runs as usual against the given `--delta`
  (if any). Every pair that is blocked ONLY by a missing `[[moved]]`
  declaration (conservation: the pass-2 pairs that are not unchanged items and
  not covered by the delta, tests included; discovery: the corresponding
  moved test identities) is written to `<path>` as TOML:
  `base = "<resolved full base sha>"` followed by one `[[moved]]` table per
  pair with `path`, `item`, `to_path`, `to_item` (for discovery: binary id /
  test name in the same fields the existing delta loader reads),
  `reason = "relocated by the layout sweep"` and
  `owner = "@layout-sweep"`. Deterministic order (sort by base path, then
  base item); no duplicates; file written only when at least one pair exists.
- Items that are really lost, really new, body-changed, signature-changed,
  attribute-changed or ambiguous (`UNPAIRABLE_DUPLICATE`) are NEVER written to
  the suggestion; they stay findings exactly as today. `TEST_LOST` for a test
  with no head counterpart stays a finding. `[[renamed]]`, `[[extracted]]`,
  `[[removed]]`, `[[added]]`, `[[changed]]` are not suggested.
- Exit code with the flag: identical to a run without it (non-zero while any
  finding, including the undeclared moves, exists), so the suggestion never
  turns a failing gate green. Print one summary line to stderr
  (`rust-guard: note: N undeclared move(s) written to <path>`).

## CI wiring (`.github/workflows/ci.yml`)

In the `source-integrity` job's "rust-guard campaign gates" step and in the
CI-03b discovery step: if the normal `conservation` / `discovery` run fails,
re-run the same command with `--suggest-delta suggested-<campaign>.toml`
(keep the original exit code as the step result) and add a final
`if: failure()` step that uploads the `suggested-*.toml` files as an artifact
named `rust-guard-suggested-delta` with `if-no-files-found: ignore`. Use the
exact pinned `actions/upload-artifact` reference already used in
`.github/workflows/release.yml` (SHA with the version comment); do not
introduce an unpinned action. Do not change what the gates accept.

## Tests

Add fixtures + tests in `tools/rust-guard/tests/` (the existing
`conservation_tests.rs` / `discovery_tests.rs` + `fixtures/` pattern): a
fixture with a moved plain fn, a moved `#[test]`, a moved test whose body
changed (must NOT be suggested, still `BODY_CHANGED`), a real loss, and an
ambiguous duplicate (not suggested); assert the suggestion file content, that
feeding the suggestion back as `--delta` makes the clean-move fixture pass
with exit 0, and that without the flag the output is unchanged. Update
`tools/rust-guard/README.md` (flag, file shape, "never suggests losses/body
changes"). The tests are executed by CI only.

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
  (`CARGO_BUILD_JOBS=4`). No `cargo test`/`build`/`run`.
- No `xargs`; no machine-specific paths; imports at the top; `mod.rs` files
  re-export only; keep every file under 1000 lines and every folder under 8
  entries (`python -I scripts/ci/check_layout.py` after the orchestrator
  stages your files is not available to you: count by hand).
- Keep the change minimal and self-contained; do not refactor unrelated code.
- Before finishing, one verifier child agent (policy pasted verbatim) reads
  the full diff of this task and confirms: the normal (flagless) path is
  byte-for-byte unchanged, nothing lost/changed/ambiguous can reach the
  suggestion file, the exit code is unchanged, the fixtures cover the cases.

## Final message

Files changed, the shape of the suggestion file (paste a sample), real exit
codes of check/clippy/fmt, and anything you could not verify.
