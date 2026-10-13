# Brief — CI tiers: fast tier for drafts, full matrix for ready PRs, concurrency and path gating

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Layout limits (CLAUDE.md, ratchet): at most 7 direct entries per folder and at most 1000 lines per source file. Existing violations are grandfathered but must not grow: put new logic into NEW files / subfolders instead of extending an over-limit file or folder, and never push a new file past 1000 lines or a folder past 7 entries.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.

## FILE SCOPE (only these paths may be edited)

- `.github/workflows/ci.yml`, `.github/workflows/numa.yml`, `.github/workflows/supply-chain.yml` (triggers, `concurrency`, job `if:` gating, one new fast job)
- NEW `scripts/ci/` folder: the affected-crates script and its tests (python 3 standard library only, no new dependency; at most 7 direct entries)
- `CLAUDE.md` (one short new section about CI tiers), `scripts/test.sh` header comment only if it describes CI, and the CI documentation page if `grep -ril "ci.yml" docs/` finds one
- NOT in scope: any Rust source, Cargo files, deny.toml, other workflows (release, perf-gate, stress-nightly, ts-e2e-nightly, hnsw-recall-matrix)

## Task

CI-2/CI-3/CI-4: make the CI runs cheaper by SEGMENTING them into a fast tier and a full tier, and stop wasting runner capacity.

WHY (measured, see the audit below): wall time of a full run is 23-69 minutes but job RUNTIME is 0.3-18 minutes; jobs wait 12-60 minutes in the runner queue because one push creates ~22 jobs (ci.yml 14 + numa.yml + supply-chain.yml) and NO workflow has a `concurrency:` group, so superseded runs of the same PR keep queued/running and hog the pool. The lever is fewer jobs per push, not faster jobs.

REQUIRED
1. `concurrency` in ci.yml, numa.yml and supply-chain.yml: group `${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}`, `cancel-in-progress: ${{ github.event_name == 'pull_request' }}` (a newer push to a PR cancels the older run; master pushes and scheduled/dispatch runs are never cancelled).
2. Triggers: `pull_request` types `[opened, synchronize, reopened, ready_for_review]` (ready_for_review is what starts the full tier when a draft is promoted), `push` to master unchanged, add `workflow_dispatch` to ci.yml.
3. FAST tier = ONE new job `fast` in ci.yml, `runs-on: ubuntu-latest`, running ONLY for draft pull requests (`if: github.event_name == 'pull_request' && github.event.pull_request.draft == true`). Steps in ONE job and ONE runner with a shared cargo cache: checkout (fetch-depth: 0), toolchain with rustfmt+clippy, rust-cache, `cargo fmt --all -- --check`, the affected-crates script, then `cargo clippy --locked --all-targets -D warnings` limited with `-p` to the affected crates (workspace on fallback), then the lib tests of the affected crates through `./scripts/test.sh --locked -p ...` (install nextest exactly as the existing test job does, pinned). Fast tier must stay green-or-red meaningful: it never silently skips (if the script says "full workspace", run with `--workspace`).
4. FULL tier = the existing jobs fmt, clippy (3 OS), test (3 OS), integration (3 OS), ts-unit, cooldown, loom-model get `if: github.event_name != 'pull_request' || github.event.pull_request.draft == false` so they run for ready PRs, for master pushes and for workflow_dispatch, and NOT for draft pushes. Keep every job `name:` unchanged.
5. Path gating (so a PR that cannot affect them does not queue them): numa.yml jobs only when the PR/push changes `crates/shamir-numa/**`, `crates/shamir-tunables/**`, `Cargo.lock`, `.github/workflows/numa.yml` (use `on.pull_request.paths` and `on.push.paths`; a workflow with a paths filter that does not match does not run at all, which is intended); the `deny` job of supply-chain.yml only when `**/Cargo.toml`, `Cargo.lock`, `deny.toml`, `.github/workflows/supply-chain.yml` change on pull requests (keep push-to-master unfiltered and keep its schedule/dispatch jobs exactly as they are); ts-unit only when `crates/shamir-client-ts/**` or `.github/workflows/ci.yml` change (use a small `dorny/paths-filter`-free approach: a first step `git diff --name-only` against the base, and `if:` on later steps; if you cannot do this without adding a third-party action, leave ts-unit ungated and say so). Draft pushes never run numa/supply-chain (add the same draft condition to their jobs for pull_request events).
6. `scripts/ci/affected_crates.py` (python 3 stdlib only): input `--base <git ref>` (default `origin/master`), reads `git diff --name-only <base>...HEAD`, reads `cargo metadata --format-version 1 --no-deps` JSON (or `--metadata <file>` for tests) and prints the sorted `-p <crate>` arguments on ONE line. Rules: a changed file under `crates/<dir>/` marks the workspace package whose manifest dir is that folder; then add ALL workspace packages that depend (directly or transitively, normal/dev/build deps among workspace members) on a marked package; if any changed path is `Cargo.lock`, root `Cargo.toml`, `rust-toolchain*`, `deny.toml`, anything under `.cargo/`, `.config/`, `scripts/`, `.github/`, or any file outside `crates/`, `docs/` and `*.md`, print the single word `--workspace`; a diff that touches only `docs/` or `*.md` prints an EMPTY line (no crates affected) and the fast job then runs only the fmt step and the script tests (document this in the script docstring). Exit code non-zero with a clear message on any failure (never print a partial list). Tests with `unittest` in `scripts/ci/tests/` (pure functions over a metadata dict and a file list: direct, transitive, dev-dependency, global-file fallback, docs-only, unknown crate folder falls back to `--workspace`); run them in the fast job and in the full `fmt` job with `python -I -m unittest discover -s scripts/ci/tests`. You MAY run `python -I -m unittest discover -s scripts/ci/tests` locally (milliseconds, no cargo); everything else is verified by CI.
7. Docs: a short section in `CLAUDE.md` ("CI tiers"): draft PR = fast tier (one ubuntu job: fmt, clippy and lib tests of affected crates); `gh pr ready` / ready_for_review / master push / workflow_dispatch = full matrix; the merge gate is the FULL tier green; how to force the full tier on a draft (`gh workflow run ci.yml --ref <branch>`); Windows/macOS-only failures surface only at ready. Update any other doc that describes the old "every push runs everything" behaviour.
8. Do NOT change what any existing job executes (commands, flags, versions, action pins): only triggers, `if:`, `concurrency` and the new `fast` job. Keep all action SHAs exactly as they are. Validate YAML by careful re-reading (indentation, expressions, `${{ }}` quoting); GitHub reports workflow syntax errors only on push, so be conservative and simple.
9. Final message: list changes per file, state how many jobs a draft push, a ready PR and a docs-only PR now create (before: ~22), and anything you could not verify.


## CI AUDIT (input)

# CI audit 2026-10-13 (last 6 CI runs: 4 PR, 1 master)

Wall time per full run: 23-69 min (median ~50). Job RUNTIME is short; the wait is the runner queue.

| job | runtime (min) | queue wait before start (min) |
|---|---|---|
| fmt | 0.3 | - |
| clippy ubuntu / macos / windows | 1.6-2.4 / 2.3-3.7 / 3.5-4.1 | ubuntu 12-50, macos 19-60, windows 1-45 |
| test lib ubuntu / macos / windows | 4.3-5.4 / 5.4-9.7 / 7.9-11.3 | same pools |
| integration ubuntu / macos / windows | 6.9 / 7.7-13.4 / 15.3-18.4 | same pools |
| cooldown, loom, ts-unit | 1.0, 1.7-2.5, 0.3 | 0.4-44 |
| numa tier1 x3 OS (separate workflow numa.yml) | not measured | same pools |
| supply-chain (separate workflow) | not measured | same pools |

Findings
1. Even ubuntu jobs wait 12-50 minutes: the runner pool is saturated, not slow. One push creates ~22 jobs (ci.yml 14 + numa.yml 4-5 + supply-chain 2-3); several PRs, master pushes and re-runs compete. Fewer jobs per push is the lever, not faster jobs.
2. Every rework round on a draft PR pays the full queue again (cancel-in-progress only helps if the next push comes before the queue drains).
3. Master pushes (after each merge) re-run the full CI + numa + supply-chain (wall 54 min), competing with open PRs.
4. numa.yml and supply-chain.yml run on every PR although most PRs touch neither numa nor dependencies.
5. Branch master is NOT protected (gh api 404): jobs skipped by `if:` cannot block a merge; the merge gate is the orchestrator's rule (all 14 CI jobs green).
6. Compile errors are found at the cheapest step (fmt/clippy 0.6-3 min on ubuntu) in nearly every failed run, i.e. an ubuntu-only fast tier would have caught them without 17 queued jobs.

