You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
ci-crit-path-1887). Read the root CLAUDE.md (binding, especially "CI tiers" and the compile/test policy),
`.github/workflows/ci.yml`, `.github/workflows/cache-warm.yml`, `.github/actions/build-test-archive/action.yml`,
`scripts/test.sh`, `docs/dev-artifacts/perf/ci-timing-baseline.md` and the pipeline tests under `scripts/ci/tests/`.
The brief below is the task; it is committed at docs/dev-artifacts/prompts/ci/20-ci-critical-path.md.

# Brief: dependency cache on the CI critical path, and no lib-test build in the integration job (CI-CRIT-PATH)

## Why (measured, from the timing artifacts of one full run that touched the engine)

The critical path of a full-tier run is `cargo test integration (windows-latest)`: 1544 s, of which the build is
746 s and the test run about 630 s.

1. **External dependencies are compiled from scratch every run.** In that job cargo's timing report shows 484
   non-workspace units (1390 s of unit time) that finish only at second 503: `aws-lc-sys` build script 203 s,
   `cranelift-codegen` 117 s, `wasmtime` 52 s, `hnsw_rs` 79 s. The job log says `No cache found.` for the Rust cache
   step and the compiler-cache statistics at the end show `Compile requests 0` (the wrapper took no request at all
   in that job). The same `No cache found.` appears in the Ubuntu integration job (there the compiler cache does
   answer: 760 hits of 797 requests) and in `test build archive (windows-latest)` (347 hits of 407); `test build
   archive (ubuntu-latest)` and `cargo clippy (windows-latest)` do restore their Rust cache ("full match").
   The cache store of the CI provider held 3125 entries and 10.54 GB at that moment, against a 10 GB limit: thousands
   of tiny compiler-cache entries next to the multi-hundred-megabyte Rust cache archives, so large archives are the
   natural eviction victims.
2. **The integration job builds lib-test binaries it never runs.** Its nextest filter selects only `kind(test)`
   binaries, yet the build includes `shamir_engine "lib" (test)` (279 s, the LAST unit of the build: starts at 467 s,
   ends at 746 s), `shamir_db "lib" (test)` (126 s) and `shamir_server "lib" (test)` (47 s). Without the lib-test
   units the same build would end at second 633 (the last integration-test unit ends at 630).

## What

An INVESTIGATION first (read the workflow files and reason from the evidence above; you cannot run CI), then the fix.

1. **Why the caches miss.** Establish from the workflow text which jobs save a Rust cache, under which key and in
   which scope, and which event populates the default-branch scope. Hypotheses to confirm or refute, each with the
   lines that prove it: (H1) the integration jobs' cache is never saved in the default-branch scope, because a push to
   the default branch reuses a green pull-request run and skips the test jobs, and the weekly cache-warm workflow
   warms only the test-archive build; a cache saved by one pull request is not visible to another; (H2) eviction at
   the size limit; (H3) on Windows the compiler-cache wrapper is not effective in the integration job (0 requests)
   while it is in the archive job: compare the two jobs' steps, shells and environment.
2. **Fix, narrowest change that makes the critical-path jobs restore dependencies:** for example a shared cache key
   between the integration job and a job that IS saved in the default-branch scope, or warming the integration key
   in the cache-warm workflow (it must then build exactly what the integration job builds), plus keeping the total
   under the limit (do not add a second multi-gigabyte archive per OS if one can be shared; say what the expected
   sizes are). Keep `cache-warm.yml` and `ci.yml` consistent (their env blocks are hashed into the key, see the
   comments there). Fix the Windows compiler-cache finding if you can prove its cause from the files; otherwise
   describe exactly what the next run's log must show to decide.
3. **Integration job builds only integration-test targets.** Make the build select integration test targets
   (cargo's `--test` selection, per selected package) instead of every test target, through `scripts/test.sh` (a
   mode or flag used only by that job; `--full` for humans must keep its meaning) so that lib-test binaries are not
   compiled there. The set of tests that RUN must be exactly the same as today (the `-E` filter is unchanged);
   `--no-tests=fail` stays. Check how doc comments, the merge gate inventory and the Python pipeline tests refer to
   that step and keep them consistent.
4. Extend `docs/dev-artifacts/perf/ci-timing-baseline.md` with the measurements above, the cause you established,
   what changed, and what the next two full runs must show (first run: cache saved; second run: restored).

## Forbidden

Changing which tests are selected or run, any timeout or budget, job dependencies (`needs`) or the merge gate's
required set; weakening `--locked`; adding unpinned actions (every action is pinned by commit SHA; prefer the ones
already used); naming any repository, account or where the pipeline runs in files of this repository (write "CI").

## Observable acceptance

YAML valid; the Python pipeline suites still hold (read them and reason; do not run them); layout passes. The
orchestrator observes in CI: `Cache restored` in the integration jobs of both OSes and in the Windows archive build
on the second full run, the Windows integration build well below 746 s, the same number of tests as before, and no
lib-test unit in the integration job's timing report.

## Constraints (binding)

Compilation, builds and tests run in CI, run by the orchestrator, never on this machine. Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build/run/bench/doc`, `npm`/`tsc`/`vitest`, wasm builds, e2e and
integration harnesses, loom, any release or full-workspace build, any command whose purpose is to build the project
or run its tests. Allowed locally, scoped to the crates you touched and nothing else:
`cargo fmt -p <crate>` (and `-- --check`), `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git (`status`, `diff`, `log`). Do not run the Python test suites either (CI runs them). Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --` with `-j 2`. A change you cannot verify here is verified by the orchestrator's CI
run: say in your final report exactly what must be observed there. If you believe you need a build or test run,
say so in the final report instead of running it.

Repo rules (read the root CLAUDE.md, it is binding): at most 7 entries per folder, at most 1000 lines per source
file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no visibility
widening, `shamir_collections::THasher` for hash maps, comments terse. Never write private machine paths, user
names, or the names of any CI infrastructure other than the workflow files into repository files. Never raise
versions. Do not touch code unrelated to the task.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: what changed (files), what you verified locally (fmt/clippy/layout only),
what is unverified and must be observed in CI, anything you decided and why. Do not end on a tool call.
