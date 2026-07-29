# Brief for F-61 (#887, P0-R2) — stop perf-gate.yml from running untrusted PR code on the self-hosted runner

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-R2) flagged that `.github/workflows/perf-gate.yml` (landed as
F-53d, commit `2fa8bba9`) triggers on `pull_request: branches: [master]`
and runs `cargo bench` (which compiles and executes the PR's own code,
including any `build.rs`/proc-macros/benches the PR modifies) on
`runs-on: [self-hosted, shamir-bench]` — a **persistent** machine, not a
disposable per-job VM.

**This is not a hypothetical future risk.** The orchestrator verified via
`gh repo view --json visibility` that this repository (`PHPCraftdream/shamir-db`)
is **already PUBLIC right now**. Any GitHub user can fork it and open a PR
against `master` today, and that PR's HEAD code is exactly what this
workflow would check out and execute. The only thing currently preventing
exploitation is that no machine is registered against the `shamir-bench`
runner label yet (per `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`, the job
just queues forever) — this task must land BEFORE that registration
happens, not after.

**User's explicit decision (AskUserQuestion, this session):** restrict the
trigger to `workflow_dispatch` (manual, maintainer-run) instead of
automatic `pull_request`. This is the simplest, safest option for the
current state — a maintainer decides when to run the gate, rather than
every fork PR triggering code execution on the persistent runner
automatically. The user was told explicitly this is not a "some day
before going public" deferred fix — the repo is public NOW.

## What to do

1. **Read `.github/workflows/perf-gate.yml` in full first.**
2. **Remove the `pull_request: branches: [master]` trigger.** Keep
   `workflow_dispatch: {}` as the only trigger. The `on:` block becomes:
   ```yaml
   on:
     workflow_dispatch: {}
   ```
3. **Update the header comment block.** The existing comment has a
   section explaining "WHY GATED ON PRs TO master ONLY (not every push,
   not feature branches)" — this reasoning is now WRONG (there is no
   longer a PR trigger at all) and must be replaced, not left stale.
   Write a new section explaining:
   - The repo is public; `pull_request`-triggered workflows check out and
     execute the PR's OWN code (any fork can supply it), and this runner
     is persistent (self-hosted), not an ephemeral VM — so automatic
     execution of untrusted PR code here is a real compromise vector, not
     a theoretical one.
   - `workflow_dispatch` requires a maintainer (someone with at least
     write access) to manually trigger a run — this is the safety
     boundary until a stronger mechanism (maintainer-approval-gated
     trusted workflow, or an ephemeral runner) is built, if ever needed.
   - Do NOT silently imply this is permanent-forever or automatically
     "solved" — note explicitly that manual-only triggering means the
     perf-gate no longer runs unattended on every PR, which is an
     intentional trade-off (safety over automatic coverage) the user
     accepted for the current state.
4. **Check for any other place that assumes/documents perf-gate.yml runs
   automatically on PRs** — in particular:
   - `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md:104` currently says "Open
     (or push to) a PR against `master`. `.github/workflows/perf-gate.yml`'s
     `perf-gate` job should now pick up on your registered runner..." —
     this describes automatic PR-triggering behavior that no longer
     exists. Update this step to describe manually triggering the
     workflow instead (`gh workflow run perf-gate.yml` or the Actions tab
     "Run workflow" button), and confirming it picks up the runner/label/
     baseline correctly.
   - `rg -n "pull_request" docs/guide-docs/CI_PERF_GATE_RUNBOOK.md
     .github/workflows/perf-gate.yml` and anywhere else referencing this
     workflow's trigger — fix every stale reference found, don't leave
     any describing the old automatic-PR behavior.
5. **Do not touch `runs-on: [self-hosted, shamir-bench]`, the build
   steps, or `./scripts/bench_gate.sh` invocation** — this task is purely
   about WHEN the workflow runs (the trigger), not what it does once
   triggered (F-60 already hardened that separately).

## What NOT to do

- Do NOT implement the maintainer-approval-gated trusted-workflow option
  or an ephemeral-runner option — the user explicitly chose the
  `workflow_dispatch`-only restriction for now. Note in your final summary
  (as a one-line forward-pointer, not an implementation) that a stronger
  mechanism may be worth revisiting later if unattended PR coverage
  becomes valuable again, but do NOT build it now.
- Do NOT touch F-55/F-56/F-57/F-58/F-59/F-60 (other tasks from the same
  review) or F-62 (wiring perf-gate into the release DAG — a separate,
  not-yet-started task that will need to account for perf-gate no longer
  auto-running on PRs, but that accounting is F-62's job, not this one's).
- Do NOT add `pull_request_target` as an alternative — that trigger runs
  in the BASE repo's privileged context with a WRITE-capable token while
  still checking out untrusted PR code if misused, which is a well-known,
  worse version of exactly this vulnerability class. Do not reach for it.

## Constraints

- This is a GitHub Actions YAML change plus a markdown doc change — no
  Rust code, no `cargo fmt`/`clippy`/tests apply here.
- Validate the YAML is well-formed (e.g. `python3 -c "import yaml,
  sys; yaml.safe_load(open('.github/workflows/perf-gate.yml'))"` or
  equivalent — check what YAML validation tooling, if any, is already
  available in this environment before reaching for something new).
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

- Read the full diff of `.github/workflows/perf-gate.yml` and
  `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`.
- Confirm no other stale `pull_request`-trigger references remain
  anywhere in the repo for this workflow.
- YAML validity check.

When done, give your final summary as plain text: the exact diff, every
stale reference found and fixed, and confirmation the YAML is valid.
