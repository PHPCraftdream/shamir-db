# Follow-up brief — P1-6 (#1019): the timeout bump was rejected — here is the real evidence, dig into THIS

## Context

Your previous pass on session `t1019-windows-ci` concluded with a fix
that simply raised `max_execution_time_secs` from 30 to 60 in the two
failing test files' server config, with a comment asserting (without
evidence) that "Windows CI runners under contention can have operations
take 30+ seconds even for trivial batches." **This has been reverted.**
It does not match the actual evidence, and it directly contradicts the
original brief's explicit instruction not to default to a timeout bump
without a concrete, evidenced justification. Do not repeat this pattern.

## The real evidence — pulled directly from the CI run this time

I fetched the actual failed-job log via `gh run view 31032501528
--log-failed` (this required `gh` — confirm it's reachable in your
sandbox; if not, this log excerpt below is sufficient, you don't need to
re-fetch it). The critical lines:

```
FAIL [140.105s] shamir-client::batch_for_each_e2e for_each_over_query_column_ref_inserts_one_audit_row_per_order_over_real_wire
FAIL [139.370s] shamir-client::batch_sequencing_e2e batch_mixed_after_and_query_edges_report_expected_provenance
FAIL [140.023s] shamir-client::batch_for_each_e2e for_each_zero_matching_orders_produces_empty_list_and_no_audit_rows_over_real_wire
FAIL [140.108s] shamir-client::batch_for_each_e2e for_each_over_literal_array_inserts_one_audit_row_per_literal_over_real_wire
...
test result: FAILED. finished in 139.98s
seed: Db { code: "limits", message: "batch execution exceeded its 30s time budget" }
```

**This is the key fact your previous pass never engaged with**: every
one of the 4 failures took a near-identical **~139-140 seconds wall
clock**, yet the error the server actually returned says the batch
"exceeded its 30s time budget." Two things follow from this, and BOTH
matter:

1. **The consistency (139.29s / 139.37s / 139.98s / 140.02s / 140.11s —
   all within under 1 second of each other, across 4 DIFFERENT test
   bodies)** is not what workload-dependent slowness looks like. Genuine
   "this operation is just slow under contention" would produce more
   variable timings across different tests doing different things. A
   near-fixed ~140s across unrelated test bodies smells like a FIXED
   timeout/retry/backoff ceiling somewhere in the stack being hit, not
   organic slowness.
2. **The reported "30s" budget vs. the observed ~140s wall-clock is the
   real puzzle to solve.** `ExecutionDeadline` (`crates/shamir-engine/
   src/query/batch/execution_deadline.rs`) is explicitly a
   **COOPERATIVE** deadline — read its module doc comment and
   `batch_execute.rs`'s comment at line 29 in full. A cooperative
   deadline is only checked at discrete checkpoints between operations,
   not preemptively. **The most likely explanation consistent with ALL
   the evidence: the batch got stuck inside a SINGLE operation (a lock
   wait, a channel recv, an await on some contended resource) for close
   to the full ~140s, and the cooperative deadline check never got a
   chance to fire DURING that stuck operation — it only fired once that
   operation finally unstuck/completed, at which point the accumulated
   checkpoint-measured time already exceeded 30s, so the error correctly
   reports "exceeded 30s" even though wall-clock was ~140s.** This
   points at a genuine STALL/lock-contention bug in one specific
   operation, exacerbated under Windows CI's resource contention — NOT
   at "the workload needs a bigger number."

I already ruled out one plausible alternate explanation for you, so
don't re-investigate it: the Rust `shamir-client` crate's
`ConnectOptions::request_timeout` **defaults to `None` (unbounded wait)**
(`crates/shamir-client/src/client.rs:675`) — these e2e tests don't set
it explicitly (confirm this is still true), so there is no
client-side-retry-loop-hitting-a-35s-timeout-N-times explanation (the
35s figure in `KNOWN_LIMITATIONS.md` is a TypeScript-SDK-specific default,
not applicable to these Rust tests).

## What to actually investigate this round

1. **Find which specific operation stalls.** All 4 failing tests are TINY
   batches (a handful of ops on a table with ≤1 rows). Instrument or
   trace (temporarily, for your own investigation — `RUST_LOG=debug` or
   `trace` scoped to the relevant modules, or add temporary
   `eprintln!`/`log::warn!` timestamps around each operation's start/end
   inside the SERVER'S batch execution path if that's more informative
   than client-side logging) to find WHICH of the batch's operations
   (create_repo? create_table? insert? read? update?) is the one that
   actually blocks for the bulk of the ~140s. You may not be able to
   reproduce the Windows-CI-contention trigger locally, but you CAN
   trace the code path and look for anything that isn't wrapped by the
   cooperative deadline's checkpoints — i.e., an `.await` that can block
   indefinitely without the batch executor getting a chance to check
   `ExecutionDeadline` in between.
2. **Look specifically at what's shared/contended across concurrent
   nextest test binaries.** These are real e2e tests — each spins up its
   own real server (`boot()` helper) with its own TCP listener + its own
   on-disk (or in-memory?) data store. Check `boot()`'s implementation
   (`crates/shamir-client/tests/` shared helper) for anything that could
   contend across CONCURRENT test processes on the same CI runner (e.g.,
   a shared temp-dir naming collision, a shared port range causing
   listener bind retries with backoff, a file lock on a shared resource).
   `cargo nextest` runs many test binaries in parallel — on a
   resource-constrained Windows CI runner, that's exactly the kind of
   condition that could turn a normally-instant lock/listener acquisition
   into a multi-second-to-multi-minute wait if there's retry/backoff
   logic with a large ceiling.
3. **Check for a literal ~140s-adjacent constant anywhere reachable from
   this code path** — a retry loop with fixed backoff × attempt count
   that would sum to ~140s, a bind-retry loop, a lock-acquire retry with
   escalating backoff. I checked the obvious `request_timeout`-related
   spots and ruled them out (see above) — check less obvious places:
   listener bind retry loops, `redb`/`fjall` file-lock acquisition retry
   loops (`crates/shamir-storage`), or anything in `ServerLauncher`/
   `boot()`'s startup sequence that could stall under contention.
4. **If you find the actual stall site**, determine whether it's:
   - A genuine bug (missing cooperative-deadline checkpoint inside a
     long-running/contended operation) — fix it by adding a checkpoint,
     or by making the underlying wait itself bounded/cancellable.
   - A test-infrastructure-only issue (e.g., the `boot()` helper's own
     startup racing with other concurrent test processes) — fix the
     test helper, not production code, and say so explicitly.
   - Genuinely something that only manifests under CI-specific
     contention you can reason about but not reproduce locally — in that
     case, propose the most targeted fix you can justify from the code
     path analysis, and say plainly that it's not been empirically
     confirmed locally, rather than presenting it as proven.

## What NOT to do

- Do not raise `max_execution_time_secs` (client or server side) as the
  fix. Even if you eventually conclude a config value needs adjusting
  SOMEWHERE, it must target the actual mechanism you found stalling, and
  the reasoning must explain the ~140s figure, not just "give it more
  headroom."
- Do not conclude "could not reproduce, bumped timeout as a safe
  default" — that was already tried and rejected.

## Constraints

Same as the original brief. Use `./scripts/test.sh`, never raw `cargo
test`/`cargo nextest run`. No stray scratch files at the repo root (your
previous pass left `test_debug.log` there — clean up after yourself).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Definition of done

- [ ] The specific stalling operation/mechanism identified via code-path
      analysis (and local tracing/instrumentation where possible), not
      guessed.
- [ ] A real fix targeting that mechanism — or an honest "investigated
      thoroughly, here's my best-evidence hypothesis, could not fully
      confirm locally" report if genuinely inconclusive.
- [ ] If a fix was made, the affected tests run repeatedly and pass.
- [ ] gates green, real output reported.
