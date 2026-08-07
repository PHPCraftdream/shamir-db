# Investigation notes — #1019 (P1-6): Windows CI batch-e2e "exceeded its 30s time budget"

**Status: root cause not conclusively identified — this documents what
was found, what was ruled out, and the diagnostic instrumentation added
to make the next occurrence traceable from CI logs alone.**

## The failure

CI run `31032501528` (`cargo test integration (windows-latest)`, job
`92396396651`): 4 tests in `crates/shamir-client/tests/` failed within
one run, each after **~139-140s wall-clock**, all reporting the same
error shape:

```
FAIL [140.105s] shamir-client::batch_for_each_e2e for_each_over_query_column_ref_inserts_one_audit_row_per_order_over_real_wire
FAIL [139.370s] shamir-client::batch_sequencing_e2e batch_mixed_after_and_query_edges_report_expected_provenance
FAIL [140.023s] shamir-client::batch_for_each_e2e for_each_zero_matching_orders_produces_empty_list_and_no_audit_rows_over_real_wire
FAIL [140.108s] shamir-client::batch_for_each_e2e for_each_over_literal_array_inserts_one_audit_row_per_literal_over_real_wire

Db { code: "limits", message: "batch execution exceeded its 30s time budget" }
```

Fetched via `gh run view 31032501528 --log-failed`. CI history shows
this recurring intermittently on unrelated commits (#946, #949-951) —
not a one-off.

## Why this is very likely a real stall, not "the workload needs 140s"

Every one of the 4 failing tests is a TINY batch (a handful of ops on a
table with ≤1 rows) — normal execution is low-milliseconds. The
near-identical ~139-140s figure across 4 unrelated test bodies (all
within under 1 second of each other) does not look like organic,
workload-proportional slowness; it looks like a shared mechanism being
hit.

## The mechanism that reconciles "30s reported" with "~140s observed"

`ExecutionDeadline`
(`crates/shamir-engine/src/query/batch/execution_deadline.rs`) is
explicitly **cooperative**, not preemptive — its own module doc states
the design intent directly:

> Deliberate non-goal: a SINGLE op that stalls forever inside one
> `.await` (a pathological I/O hang) is not interrupted by checkpoints
> — that is a different failure class (an I/O-layer liveness concern),
> and preemptively cancelling it is exactly the unsafe behaviour this
> redesign removes.

This is the key: if a single operation inside one of these batches
stalls for close to ~140s (a genuine I/O/lock-layer hang, plausibly
Windows-CI-contention-triggered), the deadline check literally cannot
interrupt it mid-stall — checkpoints only run BEFORE each stage-alias
dispatch (`batch_execute.rs:123,349,462,606`). Once the stalled op
finally returns, the NEXT checkpoint correctly observes that
accumulated elapsed time now exceeds the 30s budget and reports exactly
the error seen — `"exceeded its 30s time budget"`, even though the wall
clock reached ~140s. **The "30s" in the error message is the configured
limit, not proof of when the check fired.**

## What was investigated and ruled out (two /crush rounds + orchestrator verification)

- **Client-side retry/timeout loop**: `ConnectOptions::request_timeout`
  defaults to `None` (unbounded) in the Rust SDK
  (`crates/shamir-client/src/client.rs:675`) — these e2e tests don't
  override it. No retry loop found in the client connection logic. (The
  "35s" figure mentioned in `KNOWN_LIMITATIONS.md` is a TypeScript-SDK-
  specific default and does not apply here.)
- **A literal ~140s constant anywhere reachable from this path** —
  searched client/transport/storage/server code; none found.
- **Server boot / listener bind retry loops** — `ServerLauncher` binds
  to `127.0.0.1:0` (OS-assigned port); no retry/backoff constant found
  that would explain the figure.
- **Storage-layer file locks** — no retry loop identified in
  `crates/shamir-storage` on the paths these tests exercise.
- **Local reproduction** — all 4 previously-failing tests pass in
  1.5-2.3s locally, repeatedly, including under some concurrent load.
  This is expected if the root cause is genuinely CI-runner-contention-
  triggered (shared-runner CPU/IO contention that a single local dev
  machine, even under some artificial load, doesn't reliably reproduce)
  rather than a deterministic code bug.
- **A previous fix attempt (bumping `max_execution_time_secs` from 30s
  to 60s in the affected tests' server config) was reverted** — it
  doesn't address the actual mechanism (a stalled single op inside the
  cooperative-checkpoint gaps) and wouldn't have prevented the failure
  even on its own terms, since the observed wall-clock (~140s) already
  exceeds the proposed new ceiling's headroom rationale.

## What was NOT identified

- WHICH specific operation (within the batch: `create_repo`,
  `create_table`, `insert`, `read`, `update`, or `for_each` iteration
  dispatch) is the one that actually stalls.
- WHY it stalls under Windows CI specifically — plausible candidates
  (not confirmed): TCP/TLS handshake contention under many concurrent
  nextest test binaries each opening their own listener, a Windows-
  specific file-lock/I/O path under `shamir-storage`, or tokio scheduler
  starvation from many concurrent `multi_thread, worker_threads=4`
  runtimes competing for a CI runner's limited core count.

## What was landed this round: diagnostic instrumentation, not a fix

`crates/shamir-engine/src/query/batch/batch_execute.rs`: both per-alias
dispatch loops (`execute_plan_impl`, `execute_plan_tx_impl`) now time
each individual op and emit `log::warn!("batch op '{alias}' took
{elapsed:?} to execute...")` if any single op exceeds 1 second. This
directly targets the diagnostic gap above: the NEXT time this reproduces
in CI (with `RUST_LOG` at `warn` or above, which CI's default logging
level should already satisfy — confirm this before relying on it), the
log will show exactly which alias stalled and for how long, instead of
only the wall-clock/budget mismatch this investigation had to work
backward from.

## Next steps (not done in this round)

1. Confirm CI's logging configuration actually surfaces `log::warn!`
   output for the `windows-latest` integration job (check the test
   harness / `env_logger` init in the e2e test's `boot()` helper) — if
   warnings are suppressed, the new instrumentation is silent and needs
   a logging-level fix first.
2. Wait for/trigger a recurrence and read the new warning output to
   identify the specific stalling operation.
3. Once identified, root-cause and fix the actual stall (or, if it's a
   test-infrastructure-only issue — e.g. `boot()`'s startup racing under
   concurrent nextest binaries — fix the test helper specifically).
