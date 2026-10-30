# CI-WIN - split test build from test run, and warm the base cache

Owner decision 2026-10-30: the 30-minute limit of `cargo test lib (windows-latest)` on a cold cache is fixed by (b) a warm base cache and (c) splitting the build from the test run. Raising the timeout is not the fix. Read `CLAUDE.md` first (compile/test policy, CI tiers, layout limits), then `.github/workflows/ci.yml`, `scripts/test.sh`, `.config/nextest.toml`, `.cargo/config.toml` (runner guard), `scripts/ci/gates/merge_gate_check.py`, `scripts/ci/tests/gates/merge/`.

## Observed facts (2026-10-30)

- Fork PR run of the full tier (marker commit): `cargo test lib (windows-latest)` was cancelled by its `timeout-minutes: 30` on a cold cache. `gh run rerun --failed` on the same SHA passed; the warm run takes about 21 minutes. The other full-tier jobs are comfortably inside their limits.
- `Swatinem/rust-cache` (pinned in `ci.yml`) restores the dependency artefacts but not the workspace members, so a warm run still compiles all 26 workspace crates plus every test binary. The workspace compile and link is the dominant, OS-dependent cost on Windows.
- Hypotheses to confirm from the workflow files and the action's documented behaviour (not from memory alone): (1) the cache is saved only when the job succeeds, so a job killed by the timeout never saves and every rerun is cold again; (2) no run on the default branch ever saves a base cache, because master CI is not run in practice (upstream landing is by manual attestation; the fork's master is synced with its workflows disabled), so a PR run has no base cache to restore.
- `test` (`./scripts/test.sh --locked`, i.e. `cargo nextest run --lib --no-fail-fast`, plus `--profile ci` when `CI=true`) and `integration` (`./scripts/test.sh --full --locked -E 'kind(test)'`) each build their own test binaries. Both are gating jobs listed in `merge-gate.needs` and `REQUIRED_JOBS`.

## Goal

1. Build the test binaries once per OS in a dedicated job and hand them to the jobs that run tests, so that compile time and test time have separate budgets and a cold build no longer decides whether tests can run.
2. Make the cold case rare: a scheduled / manually dispatched workflow on the default branch builds the same thing with the same cache key, so every PR run (fork and upstream) restores a base cache.

## Required design (deviate only with a written reason in your final report)

### A. Build job + archive

- New job `test-build` in `ci.yml`, same matrix as `test` (ubuntu, windows; macOS on dispatch/schedule exactly like `test`), `needs: plan`, `if: needs.plan.outputs.full == 'true'`, `timeout-minutes: 60`. Steps: checkout, toolchain, the pinned nextest install, rust-cache (with a `shared-key` that the warm workflow also uses), then build a `cargo nextest archive` of the workspace targets that `test` and `integration` run today (`lib`, and the integration `test` targets; benches are never built). Upload it with `actions/upload-artifact` (pin by full commit SHA with a version comment, like every other action in this repo; `retention-days: 1`; one artefact per OS).
- `test` and `integration` get `needs: [plan, test-build]`, download the archive of their OS and run `./scripts/test.sh` against it. `scripts/test.sh` stays the only blessed entry point: add the archive build and the archive run as documented modes of the wrapper (for example `--archive-to <file>` and `--from-archive <file>`), keep every existing behaviour and the `--profile ci` auto-selection, keep the header usage text accurate. The test selection (`--lib` for `test`, `-E 'kind(test)'` for `integration`) and the nextest config stay exactly as they are: the same tests must run.
- The runner guard in `.cargo/config.toml` gates on `$NEXTEST`; confirm from the nextest documentation how a configured target runner applies to archived binaries and that `--workspace-remap` (or equivalent) is set so the workspace root of the extracting job is used. The wrapper must still refuse nothing that works today and allow nothing that is banned.
- The ubuntu doctest step (`cargo test --doc -p shamir-engine --locked`) is not covered by an archive: keep it where it is (ubuntu, in `test`) or move it into `test-build`, whichever keeps the cold ubuntu job inside its limit; say which and why.
- Keep the Windows `bash` shell choice for every `run:` step that calls the wrapper.

### B. Merge gate bookkeeping

- Add `test-build` to `REQUIRED_JOBS` in `scripts/ci/gates/merge_gate_check.py`, to `merge-gate.needs`, and update the docstring / counts and every unit test that encodes the job list (the inventory test `scripts/ci/tests/gates/merge/test_gate_inventory.py` reads `ci.yml`; the other tests in that folder hard-code the count). Draft fast tier: `test-build` is skipped by design like every other full-tier job.
- Folder limits: `scripts/ci/tests/gates/merge/` and `scripts/ci/gates/` must stay at most 7 entries; new Python logic, if any, goes into a new file with a test.

### C. Warm-cache workflow

- New `.github/workflows/cache-warm.yml`: triggers `workflow_dispatch` and a weekly `schedule` (off-minute cron), `permissions: contents: read`, a job guard so it only runs on the default branch (a cache saved on a feature ref is invisible to other PRs), matrix ubuntu + windows, `timeout-minutes: 90`. It runs the same build steps as `test-build` with the same rust-cache key. To make "same key" true by construction, put the shared steps in a local composite action (for example `.github/actions/build-test-archive/action.yml`) used by both `test-build` and the warm workflow; do not copy-paste the step list. The warm workflow must not upload artefacts.
- Document in `CLAUDE.md` ("CI tiers"): what `test-build` is, that `test` / `integration` consume its archive, that `gh workflow run cache-warm.yml --ref master` (in the repository whose CI is used) is run after the default branch changes `Cargo.lock` or the toolchain, and that the weekly schedule keeps it from expiring. Keep the section's existing wording otherwise.

## Non-goals

No change to which tests run, to nextest profiles or per-test timeouts, to the clippy / integration / test-discovery / loom jobs beyond what the archive hand-off needs, to dependencies, versions, or to other workflows. No `timeout-minutes` raise of `test` or `integration`; `test-build` is the only job with the long limit. Do not touch the `plan` job logic.

## Acceptance (observed by the orchestrator in CI, not by you)

1. A full-tier run (draft with the `[ci full]` marker) is green on ubuntu and windows with the same per-OS test counts as the pre-change baseline (the nextest `Summary` lines of `test` and `integration`).
2. Cold windows `test-build` finishes inside its 60-minute limit and its post step saves the cache; a rerun is warm and the Windows `test` job (extract + run) is clearly shorter than today's 21 minutes. Print the build and test wall times of each job in the job summary (`$GITHUB_STEP_SUMMARY`) so they can be read without log archaeology.
3. `merge gate (required)` is green on the marker run and still fails on a draft without the marker; the Python gate tests pass in the fast tier.
4. `cache-warm.yml` parses (`actionlint`-clean by reading, no unknown keys) and, once on the default branch, `gh workflow run cache-warm.yml --ref master` saves the cache that a later PR run restores (orchestrator verifies after landing).

## Local gates (only these)

`python -I scripts/ci/check_layout.py` (ignores untracked files: count direct entries of touched folders yourself with `ls`); `python -I -m py_compile` on every Python file you touch (a syntax check, not a test run); `cargo` is not needed because no Rust source changes. YAML and shell are checked by reading them twice for the whole error class (indentation, expression syntax, `needs` names, quoting of `${{ }}`, `shell: bash` on Windows, artefact names per matrix cell). Report real exit codes.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the files this brief names or implies (workflows, the composite action, `scripts/test.sh`, `scripts/ci/**`, `CLAUDE.md`, the nextest header comment that lists the pin call sites). No new third-party dependency beyond GitHub actions pinned by full commit SHA with a version comment (`actions/upload-artifact`, `actions/download-artifact`; reuse the SHAs already present in the repo's workflows where the same action is used). Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file.
- A claim in this brief that you cannot confirm from the repository or the pinned action's documented behaviour is reported with your own evidence, not silently changed.

## Final report

What you changed per file; the exact wrapper flags added; how the archive run interacts with the runner guard and why it is safe; where the cache key lives and how the warm workflow shares it; the gate-bookkeeping edits; any open risk that only CI can answer; anything that needs a build or test run.
