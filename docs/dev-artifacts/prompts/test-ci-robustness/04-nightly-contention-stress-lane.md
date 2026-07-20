# Test/CI Robustness 7e — nightly contention/stress lane for the Version Oracle area

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fifth item of "Этап 7 — Test / CI robustness"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 08 (`docs/dev-artifacts/research/2026-07-17-release-audit/
08-test-coverage-ci-robustness.md`, §3 "The structural problem"). This is
a CI/workflow addition — read report 08's §3 in full for the complete
reasoning before starting.

**The problem, verbatim from report 08:**

> `.config/nextest.toml:80` sets `profile.ci` `test-threads = 4` precisely
> to *reduce* worker-thread oversubscription on CI runners. That is correct
> for stability — but oversubscription is exactly the condition that
> exposed the real MvccStore deadlock (bug 3). After this cap, **no
> environment routinely recreates the contention that found the bug**: dev
> boxes have 16 idle cores, CI is now throttled. **Recommendation**: add a
> scheduled (nightly/weekly) stress job that runs `./scripts/test.sh
> @oracle --full` plus the mvcc_store_tests with deliberately high
> parallelism (e.g. `--test-threads` 2–3× cores, or the suite looped 10×),
> so contention-only deadlocks have a home where they are *expected* to
> reproduce.

The report's §3 also names the highest-risk narrow-window test files
worth being aware of (don't necessarily need per-file special-casing, but
useful context for choosing the parallelism knob): `overlay_ordering_tests.
rs`, `a10_toctou_tests.rs`, `wal_group_commit_tests.rs`,
`leader_cancel_tests.rs`, plus several spin-wait-with-no-local-timeout
sites this campaign's task 7f will separately harden.

**Confirmed via investigation**: no scheduled/nightly workflow exists yet
in this repo. `.github/workflows/` has exactly three files: `ci.yml`
(push/PR gate), `supply-chain.yml` (has a real `schedule:` cron precedent
— Sundays 03:17 UTC, `workflow_dispatch: {}` too, with a job-level `if:
github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'`
gate limiting a specific job to those triggers), `numa.yml`. `@oracle` is
already a defined scope in `scripts/test.sh` (`-p shamir-tx -p
shamir-engine`).

## The task

1. Create a NEW workflow file (mirror `supply-chain.yml`'s own top-of-file
   rationale-comment convention — this project documents WHY each job's
   cadence was chosen), e.g. `.github/workflows/stress-nightly.yml` (pick
   a clear name; check no naming collision).
2. Trigger: `schedule:` cron (nightly — pick an off-hour UTC time
   following `supply-chain.yml`'s "off the top/bottom of the hour" note)
   PLUS `workflow_dispatch: {}` for manual runs (mirroring the existing
   precedent exactly).
3. The job should run TWO things per report 08's recommendation:
   - `./scripts/test.sh @oracle --full` (the Version Oracle area:
     shamir-tx + shamir-engine, lib + integration tests) — but with
     DELIBERATELY ELEVATED parallelism relative to the normal `ci`
     profile's `test-threads = 4` cap. Decide how to override this for
     just this job: an explicit `--test-threads N` CLI flag passed through
     `./scripts/test.sh` (check whether the wrapper passes through extra
     nextest args — it should, verify), or a NEW nextest profile (e.g.
     `[profile.stress]` in `.config/nextest.toml`) with a higher
     `test-threads` value AND a longer `slow-timeout` (running with more
     concurrent threads on a runner with the SAME core count as CI's
     normal job increases realistic contention but also increases
     legitimate slowdown — don't set the timeout so tight that everything
     times out under the increased contention; use report 08's own
     "2-3x cores" framing to size this, and note that GitHub-hosted
     runners typically have modest core counts (e.g. 2-4 for standard
     Linux runners) so "cores" here means the RUNNER's cores, not this
     dev box's 16).
   - The mvcc_store_tests suite specifically (`crates/shamir-tx/src/
     tests/mvcc_store_tests/`) run with repetition (report 08's "looped
     10×" alternative) to increase the odds of hitting a narrow
     contention window even within one CI run. Decide the concrete
     mechanism: nextest's own repetition support if it has one for this
     version (check `cargo nextest --help` / the pinned 0.9.137's docs),
     or a simple shell loop invoking the wrapper N times, exiting non-zero
     on first failure.
4. Add a rationale comment block explaining: why this exists (the real
   deadlock this is designed to catch), why nightly/scheduled rather than
   per-PR (contention-hunting is slow and probabilistic, not a fast
   correctness gate), and why it deliberately runs the OPPOSITE
   parallelism policy from the `ci` profile (`test-threads = 4`'s own
   comment already explains ci's LOW-parallelism rationale — this job's
   comment should explain the inverse).
5. Failure handling: decide whether a failure here should do anything
   beyond the normal "workflow run shows red" signal (e.g. this project
   doesn't currently have Slack/email notification wiring visible in any
   workflow — don't invent one; just make sure the job fails loudly and
   `failure-output` is set to show useful diagnostics, mirroring
   `[profile.ci]`'s own `failure-output = "final"` choice or making a
   case for `"immediate"` here since a hang/timeout benefits from partial
   output as it happens).

## Out of scope

- Do NOT modify `ci.yml`, `supply-chain.yml`, or `numa.yml` — this is a
  new, separate workflow file.
- Do NOT implement task 7f's spin-wait/timeout wrapping (a separate task)
  even though report 08 mentions both in the same section — this brief
  is scoped to the scheduling/CI-lane addition only.
- Do NOT touch anything from the already-completed Этапы 1-6 or tasks
  7a/7b/7c/7d — this brief is scoped to this one new workflow.

## Verification (MANDATORY before you report done)

- Validate the new workflow file's YAML is well-formed (same pattern used
  for `ci.yml`/`supply-chain.yml` earlier in this campaign — `py -c
  "import yaml; yaml.safe_load(open('...'))"` or equivalent).
- If you add a new nextest profile, run it locally against a real (small)
  scope to confirm it parses and behaves as expected (e.g. `cargo nextest
  list --profile stress -p shamir-tx` or similar), and report the output.
- If you use a CLI `--test-threads` override instead of a new profile,
  confirm `./scripts/test.sh` actually forwards it through to nextest
  (read the wrapper script's arg-parsing to confirm, don't assume) and
  demonstrate it locally with a real (scoped, not full-workspace) run.
- Report your chosen cron time, your chosen parallelism value(s) and the
  reasoning (this repo's dev box has 16 cores; GitHub-hosted standard
  Linux runners are commonly 2-4 cores — state which assumption you used
  and why), and your chosen repetition mechanism for mvcc_store_tests.
- No local `cargo test` full-suite run is required for this task (it's a
  CI-config addition, not a source-code change) — but if you added a new
  nextest profile section, a local nextest-config-parsing check (as
  described above) is mandatory.
