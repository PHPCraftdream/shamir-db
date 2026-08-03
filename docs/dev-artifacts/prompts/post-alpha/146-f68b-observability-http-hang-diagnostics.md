# Task #922 -- F-68b: observability_http 600s hang recurs on ubuntu-latest, needs new diagnostics

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Confirmed 2026-08-02 via a real CI run (`ci.yml`, run `30757334929`,
triggered automatically on push of commit `e27ceacf`): a hang first
reported under task F-68b has NOT been resolved by F-70's earlier
lock-order-inversion fix (an earlier session hoped it might be, based on
one post-push CI run not reproducing it -- that was a false negative,
one green run is not enough evidence of a fix).

Symptom: `shamir-server::observability_http::metrics_exposes_unbounded_sentinel_when_no_byte_budget`
AND `shamir-server::observability_http::metrics_exposes_finite_byte_budget_gauges`
BOTH hang in lockstep -- `SLOW` markers appear for BOTH tests at
IDENTICAL timestamps (60s, 120s, 180s, ... 540s) and both `TIMEOUT` at
exactly 600.01xs, on `cargo test integration (ubuntu-latest)`
(`./scripts/test.sh --full --locked -E 'kind(test)'`). The lockstep
timing strongly suggests both tests are blocked on the SAME shared
resource (a lock, a shared port/socket, a global static, a shared
server fixture) rather than independently slow -- this looks like a
real deadlock/contention bug, not two coincidentally-slow tests.

## What to do

Per this repo's own established "instrument, commit, observe on real CI,
then fix" workflow (already used successfully for other F-68 clusters --
`docs/dev-artifacts/prompts/post-alpha/124-f68-cluster-d-hang-instrumentation.md`
is a good style reference if it still exists, otherwise infer the pattern
from how other hang investigations in this repo's git history were
approached):

1. Read `crates/shamir-server/tests/observability_http.rs` in full. Look
   for anything shared between `metrics_exposes_unbounded_sentinel_when_no_byte_budget`
   and `metrics_exposes_finite_byte_budget_gauges` specifically -- a
   shared test fixture/helper function, a module-level `once_cell`/`static`,
   a shared server port or bind address, a shared file path, a shared
   lock. Nextest normally runs tests in separate processes/threads with
   test-level isolation, so if these two are hanging in lockstep despite
   that, the shared thing is probably NOT simple in-process state -- more
   likely a shared OS-level resource (a fixed port both try to bind, a
   shared data directory, a file lock) or a common blocking call that both
   independently make and that itself is stuck (e.g. both are blocked
   inside the SAME upstream dependency's internal lock, or both are
   waiting on a scheduler/runtime resource that's exhausted).
2. Add `tracing::debug!`/`log::debug!` instrumentation at the entry/exit
   of whatever setup, request, and teardown steps these two tests share --
   enough that a hung run's log shows exactly which step never returns.
   Keep this instrumentation minimal and test-only if possible (avoid
   touching production code paths unless the hang is actually inside
   production code, in which case instrument there too -- that's fine,
   just keep the instrumentation itself narrowly scoped and remove/guard
   any that would be noisy on every normal CI run).
3. Commit this diagnostic addition alone if it's clean and self-contained.
4. Trigger `ci.yml` on real CI (`gh workflow run ci.yml --ref master`) and
   wait for it to complete -- given the hang takes the full 600s nextest
   timeout to surface, expect this run to take a while. If you don't have
   `gh` CLI access in your environment, stop after landing the diagnostic
   and clearly report what you added so the orchestrator can trigger and
   relay the log.
5. Once you have a log from a run where the hang recurs (high recurrence
   rate observed so far -- this is not a rare flake), read it to find the
   exact point execution stalls. Determine the actual root cause: a lock
   ordering issue, a port-bind collision under CI's specific scheduling,
   a leaked resource from a PRIOR test in the same binary that these two
   tests happen to run after, a runtime-starvation issue, or something
   else.
6. Fix the ROOT CAUSE once identified. Do NOT just raise the nextest
   timeout -- that papers over a real bug and directly violates this
   repo's own hard rule ("Hangs and test-locks are BUGS -- hunt and fix
   them, never tolerate", `CLAUDE.md`). Do NOT skip/ignore the two tests
   either without understanding why first.

## Definition of done

- Diagnostic instrumentation added and confirmed to surface useful log
  output on a real CI run where the hang reproduces.
- Root cause identified and fixed (or, if investigation proves these
  tests' own expectations are wrong somehow, corrected -- but only after
  understanding why, never as a default fallback).
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets
  -- -D warnings` clean for any changes.
- Re-run `ci.yml` (or at minimum `./scripts/test.sh -p shamir-server --
  observability_http` locally, several times, ideally under load to
  increase reproduction odds) after the fix and confirm both tests pass
  reliably -- not just once.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
