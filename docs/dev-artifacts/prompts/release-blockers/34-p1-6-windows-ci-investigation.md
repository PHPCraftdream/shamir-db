# Brief — P1-6 (#1019): Windows integration CI red — root-cause, not rerun-to-green

## Context

S.H.A.M.I.R. Database. Source: review 2026-08-05 §P1-6 + an open session
thread. CI run `31032501528` (Windows integration job): 4 batch e2e tests
failed TWICE with `Db { code: "limits", message: "batch execution
exceeded its 30s time budget" }`, each failure ~140s wall-clock. Other
jobs on the same run were green. CI history shows intermittent failures
on unrelated commits (#946, #949-951) too — this is not a one-off.

**The failing tests, all in `crates/shamir-client`:**
- `tests/batch_sequencing_e2e.rs::batch_mixed_after_and_query_edges_report_expected_provenance`
- three of the four functions in `tests/batch_for_each_e2e.rs` (the task
  doesn't name which three specifically — find this from the actual CI
  run's failure list if reachable via `gh run view 31032501528`, or infer
  from local reproduction; the four candidates are
  `for_each_over_query_column_ref_inserts_one_audit_row_per_order_over_real_wire`,
  `for_each_zero_matching_orders_produces_empty_list_and_no_audit_rows_over_real_wire`,
  `for_each_over_literal_array_inserts_one_audit_row_per_literal_over_real_wire`,
  `for_each_iteration_error_mid_loop_rolls_back_whole_tx_over_real_wire`).

## Why this is very likely a REAL bug, not "needs a bigger budget" — investigate with this prior, don't assume it away

I read `batch_mixed_after_and_query_edges_report_expected_provenance`
before writing this brief. **It is a TINY batch**: one `create_repo` +
`create_table` call, then a second batch with exactly 4 operations (a
zero-row marker query, one single-row insert, one read, one update, one
more read) against a table that has ONE row in it. A batch this small
finishing in low milliseconds is the expected baseline — `BatchLimits::
max_execution_time_secs` defaults to **30 seconds**
(`crates/shamir-query-types/src/batch/batch_limits.rs:80`), and this test
does not appear to override it (confirm this yourself — grep for
`max_execution_time_secs`/`.limits(` in the failing test files). **A
4-op batch on an empty table hitting a 30-SECOND ceiling is not "the
test is a little slow" — that's roughly 4-5 orders of magnitude beyond
what this workload should ever take.** That strongly suggests something
is genuinely STUCK (lock contention, a Windows-specific I/O stall, a
starvation/deadlock-adjacent condition under CI's resource constraints)
rather than "correctly slow." Go in assuming this is a real production
bug until you find hard evidence otherwise — do not default to "bump the
timeout" as the conclusion.

The task is explicit: **do not rerun until green, and do not silently
raise the timeout** — that would hide a regression, not fix one. The two
outcomes to actually distinguish, with EVIDENCE:
(a) the test genuinely needs a larger test-specific budget on a
constrained Windows CI runner (if so: WHY, concretely — what part of
this trivial workload is CI-environment-slow, e.g., process/connection
setup overhead unrelated to the batch content itself?), or
(b) the Windows execution path has a real, reproducible defect (lock
contention, a stall, a race) that happens to manifest as "hits the 30s
ceiling" — fix the actual defect.

## Investigation plan

1. **Try to access the actual CI run** if `gh` is available in your
   sandbox: `gh run view 31032501528` / `gh run view 31032501528
   --log-failed` (or the equivalent for whichever run is most recently
   red on the `windows` integration job) — read the FULL failure output,
   not just the summary this brief quotes. Note timestamps of individual
   operations if the test/harness logs them, to see WHERE the 140s wall
   time actually went (setup? the batch call itself? teardown?).
2. **Reproduce locally, repeatedly, under load** — this machine is
   Windows too, which helps even though it won't perfectly replicate
   CI's shared-runner resource contention. Run the specific failing
   tests via the mandated wrapper (`./scripts/test.sh -p shamir-client
   --full -- batch_mixed_after_and_query_edges_report_expected_provenance`
   and similarly for the for_each tests) in a loop (say, 20-50
   iterations) — ideally WHILE the machine is under concurrent load
   (e.g., run this alongside another CPU-heavy cargo build/test in
   parallel) to approximate CI's contention. Note whether you can
   reproduce a multi-second-plus stall even once locally. If you can
   reproduce it, that's your smoking gun — investigate what's blocking
   with whatever tracing/logging is available (this codebase uses
   `log::` extensively; there may already be relevant `log::debug!`/
   `log::warn!` sites on the lock/barrier paths these tests touch —
   enable them via `RUST_LOG` and re-run).
3. **Check what these tests actually share** — both files use
   `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` and a
   real server boot (`boot()` helper) + real TCP client connection (this
   is an e2e test, not an in-process call) — investigate whether
   `worker_threads = 4` combined with a CPU-constrained CI runner (GitHub
   Actions Windows runners commonly have fewer cores than a dev machine)
   could cause tokio scheduler starvation under concurrent test-binary
   parallelism (multiple test files/binaries running concurrently via
   nextest, each spinning up their own 4-worker multi-thread runtime +
   real TCP listeners) — this is a plausible root cause worth ruling in
   or out empirically, not just in theory.
4. **Check recent history for Windows-relevant changes** — `git log
   --oneline` around the commits the task names (#946, #949-951) and
   anything touching connection handling / TLS handshake / listener
   setup / the `boot()` test helper itself
   (`crates/shamir-client/tests/` shared helpers) for a plausible
   regression window. Also check whether this session's OWN recent work
   (#1011's `ReaderDrainGate`, #1015's DDL wiring, #1016's Batch builder
   changes) could have introduced new contention on a path these tests
   exercise — these tests use `try_update_after`/batch builder methods
   #1016 just touched; rule this in or out by checking if the failure
   predates #1016 in CI history (the task says it predates this
   session's work — #946/#949-951 are old commit numbers relative to the
   current #1048 range — so this is very likely NOT caused by this
   session's changes, but confirm rather than assume).
5. **Check for an actual hang vs. a genuine 30s-plus completion** — if
   `RUST_LOG=debug` (or similar) during a reproduced failure shows the
   batch actually progressing steadily just very slowly, that points to
   (a) (genuine slowness, needs investigation into WHY something this
   small is slow, then either fix that or give a justified test-specific
   budget). If the log shows execution reaching some point and then
   going silent until the 30s cutoff, that's a real stall/deadlock — find
   what it's waiting on (a lock guard held across an await, a channel
   with no sender, a barrier the test's own concurrency structure can't
   satisfy).

## Fix

Once you have a root cause with evidence, implement the actual fix.
Do NOT default to bumping `max_execution_time_secs` for these tests as
the resolution unless your investigation genuinely concludes (a) with a
concrete, justified reason "why 30s isn't enough for CI but the code is
fine" — and even then, prefer a MUCH smaller bump (e.g., a few seconds
of headroom) over an arbitrary large one, and say explicitly why the
chosen number is right.

Prove the fix with a clean run: after the fix, run the affected tests
several times via the wrapper (`./scripts/test.sh -p shamir-client --full
-- <test name>`, repeated) and report the results — "ran N times, 0
failures" is the acceptance bar, not "ran once and it passed."

## Constraints

- Follow this repo's mandated test wrapper — `./scripts/test.sh`, never
  raw `cargo test`/`cargo nextest run`.
- Gate: `cargo fmt -p shamir-client -- --check` (or whichever crate ends
  up touched), `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-client --full` (repeated runs per above).
- If after real investigation you cannot reproduce the issue locally AND
  cannot access the CI run logs to find hard evidence either way, say so
  explicitly and report your best-evidence hypothesis rather than
  guessing at a fix — this is a legitimate, honest outcome for a
  CI-environment-specific intermittent issue, better than a fix with no
  evidence behind it.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] Investigation performed with real evidence (CI logs and/or local
      repro and/or code-path analysis) — not guesswork.
- [ ] Root cause identified (or explicitly reported as inconclusive with
      the best hypothesis and why it couldn't be confirmed).
- [ ] A real fix implemented (not a blind timeout bump) — OR, if the
      genuine conclusion is "needs a larger test-specific budget," a
      small, justified bump with the reasoning stated.
- [ ] The affected tests proven stable across multiple repeated runs.
- [ ] fmt/clippy/test gates green, real output reported.
