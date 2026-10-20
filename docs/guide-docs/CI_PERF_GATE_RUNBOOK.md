# CI performance gates — operator runbook

## Policy

Benchmarks run **only by explicit workflow dispatch**, never automatically on
push, tags, schedules or PRs. The hot-path workflow's PR job runs only canned
Python unit tests; it does not install Rust or execute benchmarks. All builds,
tests, calibration and measurements happen in Actions, not on the owner's
machine. Do not add automatic benchmark triggers without a new operator decision.

The existing `perf-gate` workflow remains a separate dedicated **absolute** gate
on `[self-hosted, shamir-bench]`. It has not been replaced or silently moved to
hosted runners. Its historical nine-cell/25% policy and committed absolute
baseline are not evidence that all remediation debts are paid. Dispatch it via
Actions → perf-gate → Run workflow, or `gh workflow run perf-gate.yml --ref master`.
Initial absolute-baseline capture is not exposed by that existing workflow;
that remains blocked pending an independently approved CI capture mechanism.
Do not work around this by running benches locally. Runner registration and
absolute-baseline availability must be checked in the repository's Actions UI.
The original operator procedure for that dedicated machine (provisioning, runner
registration, label, baseline capture on the runner itself, ongoing operation) is
preserved unchanged in the last section of this document.

## Manual same-runner relative A/B

Actions → **hot-path-ab** → **Run workflow** → choose branch **master**:

- `runner=hosted`: `ubuntu-24.04`, advisory regression triage.
- `runner=dedicated`: owner-designated `[self-hosted, linux, shamir-bench]`,
  two independent predeclared five-pair batches (ten pairs total). Use a dedicated,
  isolated Linux runner, preferably disposable/reimaged, not the owner's desktop.
  Install Python 3.11+ on it. GitHub's runner-registration UI provides the official
  registration instructions; apply the exact labels above. Availability cannot
  be determined from source files.
- `operation=compare`: normal experiment, never calibrates.
- `candidate`: leave blank to resolve trusted master **once**, or supply an exact
  lowercase 40-character commit SHA reachable from that frozen master tip.
  Branch names, abbreviated SHAs, fork revisions and shell fragments are rejected.

CLI equivalents (these dispatch CI; they do not run benchmarks locally):

```sh
gh workflow run hot-path-ab.yml --ref master -f runner=hosted -f operation=compare
gh workflow run hot-path-ab.yml --ref master -f runner=dedicated -f operation=compare -f candidate=<full-trusted-master-SHA>
gh run list --workflow hot-path-ab.yml
gh run view <run-id> --log
gh run download <run-id>
```

A is always `e11064772960e06c24b97379ea637335a286382e`. B is the pinned resolved
candidate. Both are checked out in one job with independent absolute target
directories. Both locked release builds must finish before any measurements.
One common pinned compiler (1.95.0), crate-default features, empty RUSTFLAGS and
two Tokio/Rayon worker threads are used. No executable cache is restored.
Compiler/lockfile incompatibility is a measurement blocker, not permission to
move A forward, switch compilers on one side, or remove `--locked`.

For each cell, each batch starts with a fresh-process A/B warmup, followed by
five pairs in **AB, BA, AB, BA, AB** order. The harness additionally does its own
small untimed warmup. Processes are sequential and bounded; setup wall time is
not necessarily included in reported ns/op. Do not rerun until a favorable
result appears. The workflow has a 120-minute cap and retains immutable artifacts
for 90 days. Preserve accepted evidence in durable program records before expiry.

## Frozen counts, fixtures and calibration

The committed control checkout's `bench-iters.txt` is the reviewed iteration
manifest. It is copied byte-for-byte to both disposable CI checkouts. The selected
TOML manifest freezes common scale **1**. Digests are recorded and checked; actual
output counts must equal the cached count times scale. One-iteration cells are
smoke only and reported **inconclusive**, never certification. Counts/scale need a
separate reviewed pilot before tight claims. Missing **required** selected counts,
unexpected JIT calibration or changes to either iteration file are hard measurement
errors. Optional cells without counts are unsupported in comparison mode, not
fatal; separate calibration may select them only when both sources satisfy their
reviewed exact-cell prerequisites. Stale cached counts never prove availability.
The harness filter is a substring: selecting `/100` may also measure `/1000`;
all matching cached rows must pass strict parsing/count validation. The helper
passes the full `bench::cell` key; output still contains bare cell IDs. Milliseconds
are printed to three decimals and ns/op to two; a rounded `0.000 ms` is allowed
only with positive finite ns/op and consistent elapsed/count rounding.

Separate calibration dispatch:

```sh
gh workflow run hot-path-ab.yml --ref master -f runner=hosted -f operation=calibrate
```

This builds both revisions, then calibrates compatible selected B cells to a
one-second candidate budget. It uploads `candidate-iterations.txt` for **human
review**, never pushes or commits anything, and never compares timings or issues
certification. Review count changes and useful durations, commit accepted counts
through the normal review process, then dispatch a **new comparison** using those
frozen counts. Calibration is not re-baselining and does not write absolute
performance baselines. Failed calibration logs/partial candidate counts remain
artifacts; they are not accepted manifests.

Fixtures must be identical on both sides. This helper conservatively compares
bench-source digests and reports differing/absent fixtures as **unsupported**.
It does not copy candidate production code or bench code into A. New transactional
snapshot/vector, replication apply, n=4/8 disjoint and rejecting-sink fixtures need
reviewed frozen revision-specific adapters and explicit cached cell selections
before they can discharge debts. Optional future entries permit the workflow to
remain usable while those fixtures are absent; they are not implied coverage.
Optional entries have per-cell reviewed source-prerequisite regexes checked on
both immutable sources, with comments excluded. The current disjoint n=1 loop
cannot authorize cached n=4/8 counts. Unknown registration shapes or future
entries without explicit cell/prerequisite selections remain unsupported until
the manifest is reviewed; this is a conservative source-shape check, not a Rust
parser or proof of execution. The coarse whole-bench digest exclusion is an
explicit deviation from applying frozen revision-specific adapters: even an
unrelated bench-source difference blocks that bench rather than risking unequal
workloads. No adapters or production patches are applied by this helper.
General reads/vector adapters do not substitute for transaction-aware ranking.
AsOf remains unsupported until its repo-gate fixture is repaired. Default single
subscriber is not scaling evidence; delivery drains/unsubscribes after untimed
snapshot setup, not pure subscribe-to-Ready latency. Indexed commit workloads
may include the joined background tail, not just acknowledgment latency.

## Reading the artifacts and summary

`plan.json` records full A/B/master/control SHAs, both frozen manifest digests,
scale, selection and predeclared batch count. `machine.json` records CPU, memory,
kernel/image, compiler, source/lockfile/profile digests, features and thread policy.
Every invocation has raw stdout/stderr; successful completed processes also have
command/exit/duration metadata. Build logs survive build failure. `results.json`
and `report.json` preserve actual counts, raw observations, fixture digests,
paired ratios and statuses; `report.md` and the job summary give per-cell rows.
Selected cells blocked by build/run failures also appear as **unmeasured** rows
with the failure reason, rather than disappearing. Calibration-only cells have
unmeasured comparison rows and cannot be mistaken for benchmark results.
Reports/upload run even on failures; a platform-level force kill or failed initial
checkout may prevent complete diagnostics, so also retain the Actions job log.

The statistic is **median of paired B/A ratios**, with separate A/B ns/op medians.
Spread is `1.4826 × median absolute deviation` of paired ratios, expressed in
percentage points. These aggregate harness timings are not request p50/p95/p99.

| Class | Hosted flag margin | Dedicated initial margin |
|---|---:|---:|
| CPU projection/grouping/planning | 15% | 10% |
| In-memory async pipelines/scans | 20% | 15% |
| Contention/coordination/subscriptions | 35% | 25% |
| Buffered I/O/fsync/fjall | 40% | 25% |

These margins are policy proposals, **not measured runner noise floors**.

- **within margin**: no slowdown above the margin detected in these workloads.
  Not “no regression,” not general certification.
- **flagged**: median paired slowdown above the class margin. Hosted flags are
  advisory. Dedicated performance failure requires a quiet flag in **both**
  predeclared batches; one batch or discordant batches are inconclusive.
- **inconclusive**: spread exceeds half the margin, batches disagree, or counts
  are unsuitable for tight measurement. Investigate; do not call it a pass.
- **unsupported**: no comparable reviewed fixture; the debt remains open.
- **hard measurement failure**: build/process failure, timeout, assertion failure,
  malformed/missing/duplicate/stray data, invalid timing, unequal or unexpected
  counts, calibration or manifest mutation. Nonzero job on either runner means
  no valid answer, not necessarily a performance regression.

A green hosted job only means measurement validity, not a performance pass.
A green dedicated job also does not certify unsupported/noisy/smoke cells or disk
stability. Storage stability, runner noise, common locked release compilation,
useful durations and end-to-end future coverage require real dispatch validation.
This experiment measures the **net series effect**, not attribution to individual
commits. No local tests, builds or benchmarks were run for this implementation.

---

## Absolute perf-gate (self-hosted) — original operator procedure, preserved

This document is for the human operator who wants to run the F-53d (#877)
performance gate. **The gate is a manual, operator-run action, never an
automatic one.** `.github/workflows/perf-gate.yml` is `workflow_dispatch`
only — it does not trigger on `push`/`pull_request`, so it never queues on
a PR or blocks anything by itself; a maintainer must explicitly click "Run
workflow" (or run `./scripts/bench/bench_gate.sh` locally). It targets
`runs-on: [self-hosted, shamir-bench]`, a runner label with no machine
registered against it as of this writing — until one is registered, a
manually-triggered run will sit in "Queued". That is expected, not a bug.

> **F-62 (#888) — REVERSED (#1020, 2026-08-07, explicit operator
> decision).** F-62 had wired a release-blocking inline `perf-gate` job
> into `.github/workflows/release.yml`'s tag-triggered pipeline (same
> `./scripts/bench/bench_gate.sh` recipe, same `[self-hosted, shamir-bench]`
> runner), so every downstream release job depended on it and a `v*` tag
> push would queue the entire release pipeline until a runner was
> registered. **That job has been removed.** No benchmark runs
> automatically, on any trigger, anywhere — performance gating is
> available only as the manual action described in this runbook. Do not
> re-add a `perf-gate` job to `release.yml`'s automatic trigger without a
> fresh, explicit operator decision reversing this one.

### Why self-hosted, not a GitHub-hosted runner

Absolute `ns/op` timing gates need a stable, dedicated machine.
GitHub-hosted runners (`ubuntu-latest` etc.) are shared-tenancy and too
noisy for that — see `docs/dev-artifacts/checkpoints/2026-07-22-1747.md`,
which records 9 separate CI timing-margin flakes on hosted runners in this
repo's own history. A self-hosted box gives comparable absolute numbers
across runs, which an absolute-threshold gate depends on.

### What you need to do, in order

#### 1. Provision a machine

Your choice of hardware or VPS — this repo has no documented cloud-provider
preference today, so pick whatever you already operate (a spare
workstation, a dedicated VPS, a bare-metal box). The one hard requirement:
**it must be a machine you control exclusively for this purpose** (no other
workload contending for CPU/memory while a gate run is in flight), or the
whole point of self-hosting — stable absolute numbers — is defeated.
Minimum practical spec: whatever comfortably builds the workspace today
(see `deploy/README.md`'s resource-profile table for a general sense of
scale) plus headroom for `cargo bench`'s release-profile builds.

#### 2. Install the GitHub Actions self-hosted runner agent

Follow GitHub's own official runner setup docs (repo or org settings →
Actions → Runners → "New self-hosted runner", which generates the exact
`config.sh`/`config.cmd` command with a short-lived registration token for
your repo):

- <https://docs.github.com/en/actions/hosting-your-own-runners/managing-self-hosted-runners/adding-self-hosted-runners>

The standard flow is:

```
# Linux/macOS
./config.sh --url https://github.com/<org>/<repo> --token <REGISTRATION_TOKEN>
./run.sh

# Windows
./config.cmd --url https://github.com/<org>/<repo> --token <REGISTRATION_TOKEN>
./run.cmd
```

Run it as a persistent service (GitHub's docs cover `svc.sh install` /
the Windows service wrapper) so it survives a reboot, rather than leaving
`run.sh`/`run.cmd` attached to an interactive terminal session.

#### 3. Apply the `shamir-bench` label

`.github/workflows/perf-gate.yml`'s job targets:

```yaml
runs-on: [self-hosted, shamir-bench]
```

Apply the `shamir-bench` label either during registration (`config.sh
--labels shamir-bench`) or afterward via the GitHub UI (repo/org Settings
→ Actions → Runners → select the runner → edit labels). The label must
match this exact string — the workflow can't run without it.

#### 4. Capture the initial baseline ON that machine

Once the runner is live, capture the baseline that every future gate run
compares against. Run this ONCE, directly on the self-hosted machine (not
in a GitHub Actions job — the baseline must reflect the same physical
machine the gate itself runs on):

```
./scripts/bench/bench_gate.sh --capture-baseline
```

This runs the 9 representative bench cells (one per named workload
category — see the comment block at the top of `scripts/bench/bench_gate.sh` for
the exact list: point get, point set, scans, indexed lookup, ORDER BY
LIMIT K, commit-path proxy, FK cascade workload, cursor pages, WAL
startup/recovery) and writes `bench-baseline.json` at the repo root.

**Commit `bench-baseline.json`** (a normal file, not gitignored — analogous
to the existing committed `bench-iters.txt`):

```
git add bench-baseline.json
git commit -m "chore(bench): capture initial perf-gate baseline on <machine-id>"
```

Re-run `--capture-baseline` and re-commit whenever a genuine, intentional
performance change lands and the old baseline should no longer be treated
as the target (this is a manual, deliberate act — the gate never
auto-updates its own baseline).

#### 5. Confirm the workflow job goes green on a manual run

F-61 (#887) restricted `.github/workflows/perf-gate.yml` to
`workflow_dispatch` only — it does NOT run automatically on PRs (this repo
is public; see the workflow's own header comment for why an unattended
`pull_request` trigger on a persistent self-hosted runner is unsafe here).
Trigger it manually instead: GitHub's Actions tab → "perf-gate" → "Run
workflow" (or `gh workflow run perf-gate.yml` from a checkout with a
registered runner). It should pick up on your registered runner, run
`./scripts/bench/bench_gate.sh` (gate mode — no flags), and report pass/fail based
on the 25% regression threshold (see `scripts/bench/bench_gate.sh`'s header
comment for why 25% and how to override it via
`BENCH_GATE_THRESHOLD_PCT`). A green run confirms the runner, the label, and
the baseline are all correctly wired together.

### Ongoing operation

- **Recalibration** (not the same as re-baselining): if a bench cell's
  iteration count in `bench-iters.txt` drifts far from a useful run
  duration, recalibrate it by hand — `cargo bench -p <crate> --bench
  <bench> -- --calibrate <secs>` — same as any other bench in this
  workspace. This does not require touching `bench-baseline.json`.
- **Re-baselining** (Step 4 above, repeated): required after any
  intentional performance change (an optimization that legitimately makes
  a cell faster, or an accepted trade-off that makes it slower) so the gate
  compares against current reality instead of flagging every future PR as
  "regressed" relative to stale numbers.
- **Introducing a new workload cell** (F-60, #886): the gate is now
  fail-closed. When you add a new entry to the `WORKLOADS` list in
  `scripts/bench/bench_gate.sh`, the next gate run will FAIL because the new cell
  has no baseline entry yet — the gate does NOT silently skip cells it
  doesn't recognize. Capture a fresh baseline first
  (`./scripts/bench/bench_gate.sh --capture-baseline`), or pass
  `--allow-new-cells` for a transitional run where the new workload is
  deliberately introduced before its baseline is committed:
  ```
  ./scripts/bench/bench_gate.sh --allow-new-cells
  ```
  The same fail-closed principle applies to parser output: if a bench
  binary's stdout format drifts so the parser matches zero lines for a
  cell, or a cell appears more than once, the gate fails hard rather than
  silently dropping that cell from the comparison.
- **Threshold tuning**: the shipped 25% threshold is a conservative first
  cut (see `scripts/bench/bench_gate.sh`'s header). Once the self-hosted machine
  has run the gate enough times to characterize its own run-to-run noise
  floor, tighten `BENCH_GATE_THRESHOLD_PCT` (set as a workflow `env:` or
  repo/org Actions variable) to whatever margin comfortably clears that
  noise floor without missing real regressions.
- **Known gap — no true percentile gate**: `bench-scale-tool` is a
  fixed-iteration harness (bulk-timed, one `ns/op` per cell) — it does not
  produce a p50/p95/p99 sample distribution. The "commit percentiles"
  workload category is gated today via a single representative cell
  (`tx_pipeline::commit_tx/phases/baseline_empty`) as a proxy, not a true
  percentile measurement. Building a genuine percentile-sampling bench
  shape is out of scope for this task and is a natural follow-up.
