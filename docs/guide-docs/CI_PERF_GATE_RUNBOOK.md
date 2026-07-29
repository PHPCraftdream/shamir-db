# CI performance gate — operator runbook

This document is for the human operator bringing the F-53d (#877)
release-blocking performance gate online. **As of this writing the gate is
wired into CI but NOT yet live** — `.github/workflows/perf-gate.yml`
targets `runs-on: [self-hosted, shamir-bench]`, a runner label with no
machine registered against it. Every PR into `master` will show that job
stuck in "Queued" until you complete the steps below. That is expected,
not a bug.

## Why self-hosted, not a GitHub-hosted runner

Absolute `ns/op` timing gates need a stable, dedicated machine.
GitHub-hosted runners (`ubuntu-latest` etc.) are shared-tenancy and too
noisy for that — see `docs/dev-artifacts/checkpoints/2026-07-22-1747.md`,
which records 9 separate CI timing-margin flakes on hosted runners in this
repo's own history. A self-hosted box gives comparable absolute numbers
across runs, which an absolute-threshold gate depends on.

## What you need to do, in order

### 1. Provision a machine

Your choice of hardware or VPS — this repo has no documented cloud-provider
preference today, so pick whatever you already operate (a spare
workstation, a dedicated VPS, a bare-metal box). The one hard requirement:
**it must be a machine you control exclusively for this purpose** (no other
workload contending for CPU/memory while a gate run is in flight), or the
whole point of self-hosting — stable absolute numbers — is defeated.
Minimum practical spec: whatever comfortably builds the workspace today
(see `deploy/README.md`'s resource-profile table for a general sense of
scale) plus headroom for `cargo bench`'s release-profile builds.

### 2. Install the GitHub Actions self-hosted runner agent

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

### 3. Apply the `shamir-bench` label

`.github/workflows/perf-gate.yml`'s job targets:

```yaml
runs-on: [self-hosted, shamir-bench]
```

Apply the `shamir-bench` label either during registration (`config.sh
--labels shamir-bench`) or afterward via the GitHub UI (repo/org Settings
→ Actions → Runners → select the runner → edit labels). The label must
match this exact string — the workflow can't run without it.

### 4. Capture the initial baseline ON that machine

Once the runner is live, capture the baseline that every future gate run
compares against. Run this ONCE, directly on the self-hosted machine (not
in a GitHub Actions job — the baseline must reflect the same physical
machine the gate itself runs on):

```
./scripts/bench_gate.sh --capture-baseline
```

This runs the 9 representative bench cells (one per named workload
category — see the comment block at the top of `scripts/bench_gate.sh` for
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

### 5. Confirm the workflow job goes green on the next PR

Open (or push to) a PR against `master`. `.github/workflows/perf-gate.yml`'s
`perf-gate` job should now pick up on your registered runner, run
`./scripts/bench_gate.sh` (gate mode — no flags), and report pass/fail based
on the 25% regression threshold (see `scripts/bench_gate.sh`'s header
comment for why 25% and how to override it via
`BENCH_GATE_THRESHOLD_PCT`). A green run confirms the runner, the label, and
the baseline are all correctly wired together.

## Ongoing operation

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
  `scripts/bench_gate.sh`, the next gate run will FAIL because the new cell
  has no baseline entry yet — the gate does NOT silently skip cells it
  doesn't recognize. Capture a fresh baseline first
  (`./scripts/bench_gate.sh --capture-baseline`), or pass
  `--allow-new-cells` for a transitional run where the new workload is
  deliberately introduced before its baseline is committed:
  ```
  ./scripts/bench_gate.sh --allow-new-cells
  ```
  The same fail-closed principle applies to parser output: if a bench
  binary's stdout format drifts so the parser matches zero lines for a
  cell, or a cell appears more than once, the gate fails hard rather than
  silently dropping that cell from the comparison.
- **Threshold tuning**: the shipped 25% threshold is a conservative first
  cut (see `scripts/bench_gate.sh`'s header). Once the self-hosted machine
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
