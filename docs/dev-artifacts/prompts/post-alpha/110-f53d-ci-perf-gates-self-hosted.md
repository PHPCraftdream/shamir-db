# Brief for F-53d (#877, P2, IMPLEMENT) — CI release performance gates on a
self-hosted runner

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. F-53a/b/c/Step4 (the streaming
top-K, cursor AsOf index-seek, and FK cascade performance wave) have all
landed. This task establishes release-blocking performance gates in CI so
a future regression in any of the 8 named workload categories is caught
automatically, rather than relying on ad-hoc local bench runs.

**Decision already made by the user (do not re-litigate):** gates run on
a **self-hosted GitHub Actions runner**, not GitHub-hosted `ubuntu-latest`
— an investigation this session found GH-hosted runners are too
shared-tenancy-noisy for absolute-number timing gates (this repo's own
`docs/dev-artifacts/checkpoints/2026-07-22-1747.md` documents 9 separate
CI timing-margin flakes on hosted runners). A self-hosted machine gives
stable, comparable absolute `ns/op` numbers across runs.

**Critical scope boundary — read this before starting:** this task
(delegated to a coding agent with no cloud/physical infrastructure
access) can only implement the CI-side wiring — the workflow YAML, the
bench harness's CI-output mode, baseline persistence, and the gate logic.
**Actually registering a physical or virtual machine as a GitHub Actions
self-hosted runner is a manual, out-of-band step the human operator must
do themselves** (installing the `actions-runner` agent, registering it
against the repo/org with a runner token, applying the label this task's
workflow job targets). Do NOT attempt to provision cloud infrastructure,
call any cloud provider API, or invent credentials — there are none
available in this environment. Instead, this task's deliverable includes
a clear, step-by-step runbook document telling the operator exactly what
to do to bring the runner online, and the workflow job itself should be
written to target a specific runner label (e.g. `runs-on: [self-hosted,
shamir-bench]`) that simply won't have anywhere to run until that manual
step happens — this is expected and correct, not a bug to work around.

## What to investigate first (do not skip)

1. Read `crates/shamir-bench-utils` in full (the `Harness` /
   `bench_batched_async` / `bench` API) and `bench-iters.txt`'s exact
   format (a plain-text `bin::id = N` manifest — confirmed by this
   session's research: it is an iteration-count cache, NOT a timing
   baseline store).
2. Read `D:\dev\rust\bench-scale-tool`'s `bench-cli` binary (`history-diff`
   command) — it already renders before/after comparison tables from
   `bench-history.log` (plain-text `ns/op` + git commit + timestamp per
   line) but has NO machine-readable (JSON) output and NO automatic
   pass/fail gate. Confirm whether `bench-history.log` already exists in
   this repo or needs to be initialized.
3. List every existing bench file covering the 8 named workload
   categories (already inventoried this session — confirm still
   accurate): point get/set + scans + indexed lookup + ORDER BY LIMIT K +
   commit p50/p95/p99 all live in `crates/shamir-db/benches/engine_perf.rs`
   (plus `crates/shamir-engine/benches/order_by_pipeline.rs` for ORDER BY);
   concurrent FK workload in `crates/shamir-engine/benches/tx_concurrent.rs`
   + `fk_cascade_index.rs`; startup/recovery in
   `crates/shamir-wal/benches/wal_startup_open.rs`. **Cursor pages
   1/10/100 deep has NO dedicated bench today** —
   `crates/shamir-db/benches/changelog_read.rs` covers changelog
   pagination depth but not the F-53b cursor/index-seek path specifically;
   you may need to add a new bench file for this ONE category (mirror an
   existing bench file's `Harness` usage as the template — do not use
   Criterion, see CLAUDE.md's bench-tooling section).
4. Read `.github/workflows/ci.yml` and `stress-nightly.yml` in full for
   the existing job/permissions/checkout-pinning conventions this new
   workflow (or new job) must match (SHA-pinned actions, `permissions:
   contents: read`, `dtolnay/rust-toolchain@1.93.0`, the nextest version
   pin, etc. — copy the established pattern, do not invent a new one).

## What to implement

### 1. A CI-output mode for the bench harness (or a thin wrapper)

Add a way to emit each bench cell's result as a structured (JSON) line —
either a small addition to `shamir-bench-utils`/`bench_scale_tool`
consumption in this repo (a wrapper script/binary that runs the existing
benches and parses their stdout `ns/op` lines into JSON, if modifying the
upstream `bench-scale-tool` crate itself is out of scope — check whether
it's a local path dependency or a pinned crates.io version first via
`Cargo.toml`; if it's the published crates.io package, do NOT vendor/fork
it — write a thin repo-local wrapper instead) — one JSON object per
`{bench_name, cell_id, ns_per_op}`.

### 2. Baseline persistence + comparison

A committed baseline file (e.g. `bench-baseline.json` at the repo root,
analogous to the existing committed `bench-iters.txt`) storing the
expected `ns/op` (or an acceptable range) per bench cell, captured on the
self-hosted machine. A small script (`scripts/bench_gate.sh` or similar,
following the existing `scripts/test.sh` wrapper convention) that:
- runs the relevant benches via `CARGO_TARGET_DIR=<isolated dir> cargo
  bench` (per CLAUDE.md's bench cache isolation rule),
- compares each cell's fresh `ns/op` against the committed baseline,
- fails (non-zero exit) if any cell regresses beyond a threshold (start
  conservative, e.g. 20-25% — this is a first cut, not a precision-tuned
  gate; document the chosen threshold and why).

### 3. New GitHub Actions workflow job

A new job (either a new `.github/workflows/perf-gate.yml` or a new job
appended to an existing file — your call, but keep it release-blocking
only where appropriate, e.g. gated on `pull_request` to `master` or on
`release.yml`'s flow, not on every push) with:
```yaml
runs-on: [self-hosted, shamir-bench]
```
Matching the established SHA-pinned-action / `permissions: contents:
read` conventions from `ci.yml`. Document in a comment WHY this job will
simply not run (queue forever) until the label's runner is registered —
this is intentional, not a bug for a future reader to "fix" by switching
back to `ubuntu-latest`.

### 4. The operator runbook

A new doc, e.g. `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md` (or under
`deploy/`, follow whatever convention `deploy/VERIFY.md`/`deploy/README.md`
already establish), covering EXACTLY the manual steps a human operator
must perform, in order:
1. Provision a machine (their own choice of hardware/VPS — do not
   recommend a specific cloud provider unless the repo already has a
   documented preference; if unsure, note this as an open choice for the
   operator).
2. Install the GitHub Actions self-hosted runner agent (link to GitHub's
   own official runner-registration docs — the standard `./config.sh
   --url ... --token ...` flow).
3. Apply the `shamir-bench` label during registration (or via the GitHub
   UI afterward).
4. Run `scripts/bench_gate.sh --capture-baseline` (or whatever the actual
   script's baseline-capture mode ends up being named) ONCE on that
   machine to produce the initial `bench-baseline.json`, then commit it.
5. Confirm the new workflow job goes green on the next PR.

## What NOT to do

- Do NOT attempt to actually provision, register, or authenticate a
  self-hosted runner — no credentials/access exist in this environment.
- Do NOT fork/vendor `bench-scale-tool` itself unless investigation
  proves the repo already vendors it locally (check `Cargo.toml`'s
  `[patch]`/path-dependency sections first).
- Do NOT change any existing bench file's behavior/iteration counts —
  additive only (a new cursor-pages bench, a new CI-output wrapper, a new
  workflow job).
- Do NOT touch `.config/nextest.toml`, `ci.yml`'s existing jobs, or any
  already-landed F-46 through F-53 code.

## Constraints

- Prompt-first: this brief itself satisfies that requirement.
- Tests only via `./scripts/test.sh`, never raw `cargo test`.
- `cargo fmt`/`cargo clippy --workspace --all-targets -- -D warnings`
  clean.
- Benches via `bench_scale_tool::Harness`, never Criterion.
- Zero-trust: after landing, personally verify the gate script's
  regression detection works by temporarily inflating a baseline number
  and confirming the gate script correctly reports a regression, then
  restoring it.

## Verification the orchestrator will run

```
cargo fmt --all -- --check                 (scoped to touched crates)
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh
```
Plus a manual review of the new workflow YAML and runbook for
completeness and accuracy (no CI run against `runs-on: [self-hosted,
...]` is possible without the actual runner being live — that verification
step is the operator's, per the runbook).

When done, give your final summary as plain text: what was implemented,
file by file, the chosen regression threshold and why, confirmation
fmt/clippy/tests are clean, and an explicit restatement of what the
human operator still needs to do manually (do not imply the gate is
"live" — it isn't until the runner is registered).
