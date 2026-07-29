# Brief for F-62 (#888, P1-R3) — wire the performance gate into the release DAG

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P1-R3) found that `.github/workflows/release.yml`'s tag-triggered
release pipeline does not depend on the performance gate at all — every
downstream job (`build`, `docker`, `sbom`, `sign`, `github-release`)
`needs: [fmt, clippy, test, integration, ts-unit, ts-e2e, version-consistency]`
(verified: `grep -n "needs:" .github/workflows/release.yml`), with no perf
job anywhere in that list. A direct `v*` tag push can currently ship a
release without the performance gate ever running against that commit.

**Cross-workflow `needs:` is NOT supported by GitHub Actions.**
`release.yml`'s own header comment (lines 17-22) already documents this
constraint and the established workaround this repo uses: the `sbom` job
DUPLICATES `supply-chain.yml`'s sbom recipe inline inside `release.yml`
rather than trying to depend on the other workflow's job. **Follow this
exact same pattern for the perf gate** — do not attempt `needs:` across
workflow files, it will not work.

**Also read `.github/workflows/perf-gate.yml` in full before starting** —
F-61 (#887, landed just before this task) restricted it to
`workflow_dispatch` only (no automatic trigger), because this repository
is public and the self-hosted runner is persistent (see that file's own
header comment for the full reasoning). **No runner is registered against
the `shamir-bench` label yet** — any job using `runs-on: [self-hosted,
shamir-bench]` today queues forever. This is expected and out of scope to
fix (see `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`) — but it means the job
you add in this task WILL queue forever on a real tag push until a human
operator registers the runner. That is an intentional, honest consequence
of making the release gate depend on the perf gate — document it clearly,
do not work around it or silently make the dependency optional to avoid
the queue.

## What to do

1. **Add a new `perf-gate` job inside `release.yml`**, mirroring how
   `sbom` duplicates `supply-chain.yml`'s recipe: same `runs-on:
   [self-hosted, shamir-bench]` as `.github/workflows/perf-gate.yml`, the
   same checkout/toolchain/cache steps, and the same
   `./scripts/bench_gate.sh` invocation (gate mode, no flags — this
   compares against the committed `bench-baseline.json`, which does not
   exist yet either; that's fine, F-60's hardened parser will fail
   loudly with a clear "no baseline found" message rather than silently
   passing — confirm this by reading `scripts/bench_gate.sh`'s current
   gate-mode behavior when `bench-baseline.json` is absent).
2. **Add `perf-gate` to the `needs:` list of every job that currently
   needs the 7-job gate** (`build`, `docker`, `sbom`, `sign`, and
   `github-release`'s effective gate — check the exact current lists via
   `grep -n "needs:" .github/workflows/release.yml` first, since the
   brief above already captured them, but verify nothing changed).
3. **Document the queue-forever consequence explicitly** in a comment
   near the new job and/or in the top-of-file header comment (mirroring
   the style of the existing CR-B9 paragraph): a tag push will not
   complete the release pipeline until a self-hosted runner is
   registered against `shamir-bench` — this is intentional
   (release-blocking means release-blocking), not a bug, and matches
   `perf-gate.yml`'s own now-documented "queues forever until registered"
   behavior.
4. **Update `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`** if it discusses
   what depends on the gate passing — add a note that `release.yml`'s tag
   pipeline now also depends on it (a tag push will queue on the same
   unregistered-runner condition as a manual `perf-gate.yml` dispatch).
5. **Check `docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md`
   §7 "Definition of done for first tag"** — it already lists "perf-gate
   прошёл на том же frozen SHA" as a release checklist item; this task
   makes that a structurally ENFORCED gate rather than just a checklist
   line. Do not edit that document (it's a point-in-time planning
   record), just be aware your change is exactly what that line asks for.

## What NOT to do

- Do NOT attempt any cross-workflow `needs:`, `workflow_call`, or
  `repository_dispatch` trick to make `release.yml` literally depend on
  `perf-gate.yml`'s own workflow run — follow the established
  duplicate-the-job-inline pattern instead (see `sbom`).
- Do NOT change `perf-gate.yml` itself (F-61 already landed its
  `workflow_dispatch`-only restriction; don't revert or alter it).
- Do NOT attempt to register a runner or capture `bench-baseline.json` —
  both remain manual, out-of-scope operator steps.
- Do NOT touch F-55 through F-61 or F-63/F-64 (other tasks from the same
  review/wave).
- Do NOT weaken the gate to make CI "green" by skipping it when no
  runner exists (e.g. `continue-on-error: true` on the new job) — that
  would defeat the entire point of this task. A queued/blocked release
  pipeline until the runner is registered is the CORRECT, intended
  outcome right now, not a problem to paper over.

## Constraints

- This is a GitHub Actions YAML change (plus possibly one doc-comment
  update) — no Rust code, no `cargo fmt`/`clippy`/tests apply.
- Validate the YAML is well-formed after your edit (e.g. `python -c
  "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"`
  — confirmed available in this environment).
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

- Read the full diff of `.github/workflows/release.yml` (and the runbook
  if touched).
- Confirm every downstream job's `needs:` list includes `perf-gate`.
- YAML validity check.

When done, give your final summary as plain text: the exact diff, the
full updated `needs:` list for each downstream job, and confirmation the
YAML is valid.
