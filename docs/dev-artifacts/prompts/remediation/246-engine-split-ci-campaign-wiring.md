# 246 — engine-split-ci-campaign-wiring (engine crate split #1548, stage C0: make rust-guard campaigns usable for a cross-crate move; CI scripts + workflow only)

Context (@oh ruling 2026-10-29, E_PLAN §6 "C0" + §10). The first real conservation campaign is S1 (b247, `shamir-engine-query`). Re-verified at origin/master fd5d4c15, the campaign wiring has five gaps; C0 closes them and touches NO Rust:
1. `test-discovery` (ci.yml ≈ l.839–1019) lists only `cargo nextest list -p shamir-engine` at base and head and runs `rust-guard discovery` WITHOUT `--delta` → every campaign that moves a test is a deterministic `TEST_LOST`, and a test moved into another package is never listed at head.
2. One delta file cannot serve both gates: `discovery` reuses the conservation delta parser and reports EVERY entry that pairs no test as `UNUSED_ALLOWANCE` (`tools/rust-guard/src/discovery_cmd/run.rs`, loop over `d.all()`); conservation reports test-identity entries (`path = "<binary id>"`) the same way. → each campaign gets a second file, `discovery_delta`.
3. Lifecycle: `rust_guard_select.py` reads `.rust-guard/stages.toml` from the PR head and selects every campaign whose packages the diff touches. An entry still present on master after its PR lands makes every later PR touching those packages run conservation against a stale `base` (`BASE_MISMATCH`) — and removing the entry inside its own PR switches the gate off on that PR. Rule (decided): **a campaign is evaluated only on a PR whose diff touches its `delta` or `discovery_delta` file.** A landed entry is inert; the next campaign's PR removes it (S2 removes S1's).
4. `source-integrity` "rust-guard campaign gates" runs under `set -e`: when `structure` fails, `conservation` (and its `--suggest-delta` artifact) never runs → one wasted CI round per structure finding.
5. `test-discovery` runs only on ready PRs, so a wrong discovery delta costs a full-matrix cycle (hours). It must also run on drafts (ubuntu × default config only).

Read first: `.github/workflows/ci.yml` jobs `source-integrity` and `test-discovery` (whole jobs), `scripts/ci/rust_guard_select.py`, `scripts/ci/tests/test_rust_guard_select.py`, `.rust-guard/stages.toml`, `tools/rust-guard/README.md` (sections "Delta file", "Suggesting moves", "`rust-guard discovery`", "When the gates run"), `scripts/ci/gates/merge_gate_check.py` (draft-tier rules: a job that SUCCEEDS on a draft is fine — no change there), `scripts/ci/tests/gates/test_gate_inventory.py` (job ids and the merge-gate `needs:` list must stay unchanged), `CLAUDE.md` sections "rust-guard source-integrity gates (CI)" and "Compiled gates + merge gate".

## Changes
1. `scripts/ci/rust_guard_select.py` (stays < 1000 lines; `scripts/ci/` already has 7 entries — add NO new file there):
   a. `load_campaigns`: `discovery_delta` is a REQUIRED string per campaign (missing/non-string → `ValueError` → exit 2, like `delta`).
   b. New pure function `touches_own_delta(campaign, changed) -> bool`: true iff the separator-normalised `delta` or `discovery_delta` path is in the (normalised) changed list. `main` filters campaigns with it BEFORE calling `select()`. `select()` keeps its signature and its 4-tuple return value, so the existing `select` tests stay valid.
   c. Output line gets a 5th column, appended at the END: `<name>\t<delta>\t<pkgs>\t<dirs>\t<discovery_delta>` (main looks the campaign up by name).
   d. New mode `--present-in <metadata.json> --packages a,b`: prints the comma-joined subset of the given package names that exist in that `cargo metadata` JSON, input order kept; exit 2 when none is present, the file is unreadable or the JSON is bad. In this mode `--stages`/`--metadata` are not required; in selection mode they still are, and a missing argument still exits 2.
   e. Module docstring: CLI (both modes), the lifecycle rule, the 5 columns.
   Tests (`scripts/ci/tests/test_rust_guard_select.py`): `STAGES` gains `discovery_delta` on both campaigns; every `MainTest` expectation that prints a TSV line gets the 5th column and its `changed` list includes that campaign's `delta` path; add: packages touched but neither delta file touched → nothing printed; only `discovery_delta` touched → selected; campaign without `discovery_delta` → exit 2; `--present-in` (subset + order kept, none present → 2, bad JSON → 2). Follow the file's injection style (`main(argv, stages_text=…, metadata_text=…, changed=…)`); no git, no cargo.
2. `ci.yml` job `source-integrity`, step "rust-guard campaign gates":
   - `while IFS=$'\t' read -r name delta pkgs dirs ddelta; do` (`ddelta` unused here; without it the 5th column would be glued onto `dirs` and break `--scope`).
   - Run `structure` and `conservation` independently per campaign: capture each exit code (`cmd || rc=$?` under `set -euo pipefail`), keep the existing `--suggest-delta` re-run on a conservation failure, and exit with the first non-zero code only after BOTH ran.
3. `ci.yml` job `test-discovery`:
   a. Remove the draft exclusion from the job `if:` (the job runs on every event). Matrix: on a draft PR `os = ["ubuntu-latest"]` and only the `default` config; otherwise unchanged. Use `fromJSON(github.event.pull_request.draft && '<draft list>' || <current expression>)` for both `os` and `config` (`github.event.pull_request.draft` is null off-PR, so non-PR events keep today's matrix).
   b. Config `test-util-fts` flags → `--features shamir-engine/test-util,shamir-engine/fts_indexed_long` (package-qualified: valid with several `-p`, same resolution for the engine-only case).
   c. Step "format smoke": add `&& github.event.pull_request.draft != true` to its `if:` (drafts with no campaign then cost ~1 runner-minute).
   d. Step "campaign selection": keep `campaigns.tsv`; more than one row → `::error::test-discovery supports one campaign per PR` and exit 1; export step outputs `campaigns=yes`, `name`, `pkgs`, `ddelta` (from the single row).
   e. Base-list cache key: append `-${{ steps.select.outputs.pkgs }}` (a different package set over the same base must not reuse a list).
   f. Step "base list": inside the merge-base worktree run `cargo metadata --no-deps --format-version 1 > "$RUNNER_TEMP/base-metadata.json"`, then `python3 -I "$GITHUB_WORKSPACE/scripts/ci/rust_guard_select.py" --present-in "$RUNNER_TEMP/base-metadata.json" --packages "$PKGS"` — the HEAD copy of the script (the base worktree predates `--present-in`); build one `-p <name>` per present package (`IFS=',' read -ra …`); `cargo nextest list "${args[@]}" ${{ matrix.config.flags }} --message-format json`. A package added by the campaign (absent at base) is simply not listed at base.
   g. Step "head list": one `-p` per package of `$PKGS`.
   h. Both `rust-guard discovery` invocations get `--delta "$DDELTA"`.
   i. Pass step outputs to scripts through `env:` (`PKGS: ${{ steps.select.outputs.pkgs }}`, `DDELTA: …`), not inline in the script body.
   j. Rewrite the job's header comment: all campaign packages listed at base (those that exist) and head, the campaign's `discovery_delta`, the draft tier.
   Job ids, job names, `needs:` of `merge-gate` and every other job: unchanged.
4. `.rust-guard/stages.toml` header comment: document `discovery_delta` (required; shape = the conservation delta file, entries read as `path`/`item` = base binary id/test name, `to_path`/`to_item` = head ones, both explicit), and the lifecycle rule from Context item 3; the commented example gains a `discovery_delta` line. Keep `campaigns = []` (b247 replaces it).
5. `tools/rust-guard/README.md` "When the gates run": two sentences (the lifecycle rule; discovery uses the campaign's `discovery_delta`). `CLAUDE.md`: the `test-discovery` bullet (every package of the selected campaign at base and head, `discovery_delta`, drafts run ubuntu × default) and one sentence in "rust-guard source-integrity gates" on the lifecycle rule. Nothing else in these files.

## Tests (CI runs them — never locally)
The unittest suite runs in CI (`fmt` and `fast` jobs: `python3 -I -m unittest discover -s scripts/ci/tests`). With `campaigns = []` every campaign step stays a no-op notice, so on this commit alone C0 is proven only on the no-op path; the active path is proven by S1 (b247) in the same PR. Acceptance by reading:
- `grep -n 'read -r name delta pkgs dirs' .github/workflows/ci.yml` → every hit ends in `ddelta`.
- `grep -n 'nextest list -p shamir-engine' .github/workflows/ci.yml` → only the loom job's line is left.
- `grep -n 'rust-guard discovery' -A6 .github/workflows/ci.yml` in `test-discovery` → both comparing calls carry `--delta "$DDELTA"`; the format-smoke calls keep their scratch delta.
- `git diff -- .github/workflows/ci.yml` touches only the jobs `source-integrity` and `test-discovery`; job keys and the `merge-gate` `needs:` line unchanged (that is what `test_gate_inventory.py` asserts).
- `rust_guard_select.py`: `select()` signature unchanged; `--present-in` documented; every pre-existing test still present (count `def test_` before/after: only additions).

## Rules
- Touch only: `.github/workflows/ci.yml`, `scripts/ci/rust_guard_select.py`, `scripts/ci/tests/test_rust_guard_select.py`, `.rust-guard/stages.toml`, `tools/rust-guard/README.md`, `CLAUDE.md`. No Rust, no `tools/rust-guard/src/**` (tool semantics stay as they are), no new files.
- Layout: ≤ 7 entries/folder, ≤ 1000 lines per `.py`/`.sh`/`.rs` file. Comments laconic. Python stdlib only, `python3 -I` compatible.
- Compile / build / test policy (CLAUDE.md, verbatim allowed list): banned unless the user explicitly asks in the current conversation — `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t`/`cargo tl`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest, WASM builds, e2e/integration harnesses, loom, and any command whose purpose is to build the project or execute its tests (this includes `python -m unittest` on `scripts/ci/tests`). Allowed locally, scoped: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` / `-- --check`, `python -I scripts/ci/check_layout.py`. If you believe a run is needed, say so in the final message.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` / `mv`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

## Local gates (only these)
- `python -I scripts/ci/check_layout.py`
Report every changed file, the before/after `def test_` counts of `test_rust_guard_select.py`, the new TSV column order, and the real exit code.
