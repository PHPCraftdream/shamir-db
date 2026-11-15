# CI timing baseline (engine split)

Baseline record for the CI timing work: it states what the pipeline measures
and holds the before/after numbers for two upcoming changes — the split of
`shamir-engine` into several crates and the Windows integration-job
speed-up. The timing artifacts below are new as of the CI-TIMINGS change
and are kept for 14 days only, so numbers must be copied into this
document soon after each run.

---

## What is recorded and where

| Artifact | Produced by | Content | Retention |
|---|---|---|---|
| `cargo-timings-test-archive-<runner-os>` | `test build archive (<os>)` | cargo's `--timings` report from `target/cargo-timings/` (`cargo-timing.html`), emitted by the shared archive action `.github/actions/build-test-archive/action.yml`; produced also by the weekly cache-warm workflow run (full-workspace build, the cleanest build sample) | 14 days |
| `cargo-timings-integration-<os>` | `cargo test integration (<os>)` | same report, from the in-place build | 14 days |
| `junit-lib-<os>` | `cargo test lib (<os>)` | nextest JUnit XML (`junit-cargo-tests.xml`, written by nextest under `target/nextest/ci/` via the new `[profile.ci.junit]` table in `.config/nextest.toml`; the CI runner selects the `ci` profile automatically) | 14 days |
| `junit-integration-<os>` | `cargo test integration (<os>)` | same JUnit config | 14 days |

`<os>` is `ubuntu-latest` / `windows-latest` / `macos-latest` (macOS cells
only on dispatch/schedule runs). The archive-build artifact uses the
runner OS instead: <runner-os> is Linux / Windows / macOS (same composite
action, unique per OS even for callers that pass no matrix label). Every
upload runs with
`continue-on-error` and `if-no-files-found: ignore`: a missing report or a
failed upload can never fail a job. A fully cached build may produce no
timings report at all. The JSON timings variant exists only on nightly
toolchains; the workflow pins a stable toolchain, so HTML-only today. The
build-side flag is opt-in through `SHAMIR_CARGO_TIMINGS=1` in the two
build paths of `scripts/test.sh` (archive and run modes);
`--from-archive` builds nothing and ignores it. The pre-existing
human-readable timing lines in the run summary ("test archive built in
N s", "lib tests ran in Ns") remain unchanged; the artifacts are the
machine-readable layer above them.

---

## Reading the timings report

Open `cargo-timing.html` in a browser. Every compile unit is a box on a
timeline: left edge = start, width = wall-clock duration. Lines/arrows
between boxes are dependency edges; a long chain of dependent boxes is a
sequential compile chain — exactly where the `shamir-engine` →
`shamir-engine-query` → `shamir-engine-validator` → `shamir-db` →
transports → `shamir-server-security` → `shamir-sync` → `shamir-server`
chain shows up. Clicking a unit shows its details (duration, rustc
invocation, fingerprint/queue wait). The concurrency graph shows how many
build processes ran in parallel — wide gaps mean idle capacity. Color
distinguishes unit kinds (build scripts, proc macros, regular crates).

---

## Before the split

One full-tier run of an engine-touching PR (single run, not a median).
The "after" columns stay empty until the split lands.

| Metric | Windows before | Linux before | Windows after | Linux after |
|---|---|---|---|---|
| `cargo test integration` — build | 12m15s (735 s) | 6m46s (406 s) | | |
| `cargo test integration` — test run | 632 s | 96 s | | |
| `cargo test integration` — total | ≈1367 s (≈22m47s) | ≈502 s | | |
| `test build archive` | 784 s | 593 s | | |

The critical path of a full run is the Windows integration job (build +
run). The workspace compile above `shamir-engine` is one long sequential
chain, visible per crate in the timings report.

---

## Per-binary test time (sum of test times, seconds)

| Test binary | Windows | Linux | Windows / Linux |
|---|---|---|---|
| shamir-server::e2e | 622 | 102 | ≈6.1 |
| shamir-server::admin | 434 | 91 | ≈4.8 |
| shamir-db::schema_ddl | 364 | 20 | ≈18.2 |
| shamir-e2e::matrix | 339 | 30 | ≈11.3 |
| shamir-e2e::batch | 141 | 8 | ≈17.6 |
| shamir-e2e::interner | 90 | 6 | 15.0 |
| shamir-e2e::cursor | 68 | 11 | ≈6.2 |
| shamir-e2e::internals | 65 | 5 | 13.0 |
| **Total** | **2123** | **273** | **≈7.8** |

The same tests are ~7–18× slower on Windows. After the split, refill
these numbers from the `junit-*` artifacts.

---

## Median full-run duration (method)

Data source: the CI provider's run list. Exact commands (fields matter);
≥5 and ideally 10–20 runs; successful runs only. Master pushes always run
the full tier — for ready PRs use `--event pull_request`.

```
gh run list --workflow ci.yml --branch master --event push --limit 20 \
  --json databaseId,displayTitle,startedAt,completedAt,conclusion
```

Median duration in seconds over the successful runs:

```
gh run list --workflow ci.yml --branch master --event push --limit 20 \
  --json startedAt,completedAt,conclusion \
| jq '[.[] | select(.conclusion == "success")
       | (.completedAt | fromdateiso8601) - (.startedAt | fromdateiso8601)]
      | sort | .[(length / 2) | floor]'
```

Per-job durations and the critical path of one run (also
`gh run view <run-id> --json jobs` for
`.jobs[].steps[].startedAt/.completedAt` to re-split a job into build vs
run):

```
gh run view <run-id> --json jobs \
  --jq '.jobs[] | select(.conclusion != null) | [.name, .startedAt, .completedAt] | @tsv'
```

Rules: use `startedAt` (runner start), not `createdAt` (queued), as the
start of a run; compare only runs of the same event class (master push vs
PR); discard runs cancelled by the concurrency group
(`conclusion == "cancelled"`); record the median next to the "after"
columns above when the split lands.

---

## Round 2 — CI-CRIT-PATH: dependency cache on the critical path, integration build scope

### Measured (one full-tier run that touched the engine)

- Critical path: `cargo test integration (windows-latest)` — 1544 s total,
  build 746 s, test run ≈630 s.
- The build compiled 484 non-workspace units (1390 s of unit time) that
  finish only at second 503: `aws-lc-sys` build script 203 s,
  `cranelift-codegen` 117 s, `wasmtime` 52 s, `hnsw_rs` 79 s.
- Lib-test binaries were built but never run (the `-E 'kind(test)'` filter
  cannot select them): `shamir_engine "lib" (test)` 279 s — the LAST build
  unit (starts at 467 s, ends at 746 s), `shamir_db "lib" (test)` 126 s,
  `shamir_server "lib" (test)` 47 s. Without them the same build ends at
  second 633 (the last integration-test unit ends at 630).
- Rust cache (rust-cache): `No cache found.` in `cargo test integration` on
  BOTH OSes and in `test build archive (windows-latest)`; "full match" in
  `test build archive (ubuntu-latest)` and `cargo clippy (windows-latest)`.
- Compiler cache (sccache): ubuntu integration 760 hits of 797 requests;
  windows `test build archive` 347 of 407; windows integration
  `Compile requests 0` — the wrapper took no request at all while 484
  dependency units compiled.
- Cache store at that moment: 3125 entries, 10.54 GB against a 10 GB limit —
  thousands of tiny compiler-cache entries next to the multi-hundred-megabyte
  Rust cache archives, so the large archives are the natural eviction victims.

### Cause

**The integration jobs' cache key is never populated on the default-branch
scope (confirmed).** The `integration` job's rust-cache step had no
`shared-key`, so its entry used the automatic key (OS + rustc/cargo versions +
Cargo.lock hash + CARGO_*/RUST_* env). Which events could save such an entry
where every branch can see it?

- A master push skips the test jobs (`test-build`, `test`, `integration`,
  `loom-model`, `test-discovery` are gated on `tree_green != 'true'`): when a
  green same-tree PR run exists, nothing runs and nothing saves. Only a
  reuse failure runs `integration` on master.
- The weekly `ci.yml` schedule selects only `shamir-server` + `shamir-e2e`,
  so it saves an integration-keyed entry at most once a week — and in the
  measured run none was present.
- A PR run saves under the PR's branch scope, invisible to every other PR and
  to master.

`cargo clippy` has NO `tree_green` gate — it runs on every master push and
routinely refreshes its default-key entry in the default-branch scope. That is
exactly the observed split: `cargo clippy (windows-latest)` restored while
`cargo test integration (windows-latest)` found nothing. The entry class that
IS routinely present on the default branch is the shared key
`test-archive-v1` (composite action `.github/actions/build-test-archive`),
saved by `test-build` on every full run and warmed weekly by
`cache-warm.yml` — which is why `test build archive (ubuntu-latest)` restored
"full match".

**Eviction at the size limit (confirmed active).** The store sat at 10.54 GB
of a 10 GB limit; the CI provider must evict continuously. `test build archive
(windows-latest)` missed although cache-warm warms exactly its shared key on
the default branch — the windows archive (the largest archive class in the
store) had been evicted or had expired between warm and run. The fix therefore
must not add any new large archive class.

**Windows compiler cache at 0 requests (unresolved from the files).** The
step plumbing is identical between `integration` and `test build archive`
(same workflow env block, same pinned toolchain/nextest/rust-cache/sccache
steps, the same `enable sccache` bash step writing `SCCACHE_GHA_ENABLED` /
`RUSTC_WRAPPER` to `$GITHUB_ENV` before a `shell: bash` build step calling
the same `./scripts/test.sh`). Nothing in the workflow text can make cargo
see the wrapper in one job and not the other. Decision ladder: the
integration run step now prints `printenv RUSTC_WRAPPER SCCACHE_GHA_ENABLED`,
`command -v sccache` and `sccache --show-stats` after the run (never gates).
If the next run's windows integration stats again show 0 requests while
hundreds of units compiled, the wrapper is demonstrably not reaching cargo in
that job and the following change must instrument the build invocation
itself; if requests > 0 appear, the earlier 0 was situational.

### What changed

1. `integration` restores the shared `test-archive-v1` rust-cache entry —
   the same entry `test-build` saves on every full run and cache-warm warms
   weekly on the default branch. No new archive class: the shared-key entry
   keeps its current size per OS (hundreds of MB; the windows one the
   largest), saves with an existing key are skipped by the cache service, and
   integration's two default-key saves per full run cease.
2. The integration build compiles only integration-test targets: the matrix
   helper emits a new `integration_targets` output (`--test <name>` selectors
   derived from the same target sets as the `-E` filter), the job passes them
   to the new `./scripts/test.sh --integration` mode (fail-closed without
   selectors), and lib-test binaries are no longer built there. The tests
   that RUN are exactly the same: the `-E 'kind(test)'` filter is unchanged,
   `--no-tests=fail` stays.

### What the next two full runs must show

- First run: `test build archive (<os>)` saves the shared-key entry for the
  branch (its rust-cache step logs a save); if the default-branch warm entry
  is present, `cargo test integration (<os>)` already restores it. The
  integration timing reports contain NO `"lib" (test)` unit, and the test
  counts match the baseline (`junit-integration-<os>`).
- Second run: the cache restores ("full match") in `cargo test integration`
  on BOTH OSes and in `test build archive (windows-latest)`; the windows
  integration build is well below 746 s (the measured run's dependency units
  alone finished at second 503, and the lib-test units cost 452 s of unit
  time); test counts unchanged; the sccache diagnostics decide the Windows
  wrapper question per the ladder above.
