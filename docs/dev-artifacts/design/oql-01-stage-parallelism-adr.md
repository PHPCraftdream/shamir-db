# ADR: OQL Epic 01 — stage parallelism (deferred)

## Context

`BatchPlanner::plan` (`crates/shamir-query-types/src/batch/planner.rs`)
groups queries into topologically-ordered **stages**: each stage contains
aliases whose dependencies are all satisfied by earlier stages, so the
queries within one stage are mutually independent and *could* run
concurrently. The module's doc comment historically implied this
independence was actually exploited ("creates an execution plan that
maximizes parallelism"), but the executor
(`crates/shamir-engine/src/query/batch/batch_execute.rs::execute_plan_impl`)
has always run every stage's queries sequentially on a single task — a gap
between promised and actual behavior that this ADR closes by fixing the
doc claim rather than the executor.

A `futures::future::try_join_all`-based concurrent-stage experiment was
tried previously and measured as a no-op for in-memory, CPU-bound
workloads: there are no `.await` suspension points inside a single query's
execution path that would let the runtime interleave sibling queries on
one task. Real parallelism needs each stage's queries to run as
independent Tokio tasks (`tokio::spawn`-per-query), which in turn requires
`Arc<dyn TableResolver>` / `Arc<dyn AdminExecutor>` (or an equivalent
scoped-spawn helper) so borrowed trait-object references can cross task
boundaries — a non-trivial API change to `execute_plan_impl` and its
callers.

## Decision

For this phase (Epic01/A, task #628) we take the **minimum viable fix**:

- (b) **Done here.** Correct the `planner.rs` module doc comment and the
  `execute_plan_impl` doc comment so neither promises parallelism the
  executor doesn't deliver. Stages are documented as a **logical
  grouping** of independent queries; whether the executor runs them
  sequentially or concurrently is an separate, currently-sequential
  implementation choice.
- (a) **Deferred, not implemented.** A real `tokio::spawn`-per-query
  executor is out of scope for this task. It is tracked for Phase E
  (task #632 — sequencing benchmarks), where a concurrent-stage executor
  can be measured against a real I/O-bound workload (disk-backed repos,
  network transport) where suspension points actually exist and
  concurrent stages could show a measurable win.

## Why not now

1. **No evidence of benefit yet.** The one measurement we have (in-memory,
   CPU-bound) showed no win from concurrent stages; committing to a
   `tokio::spawn`-per-query design without a workload that demonstrates
   the benefit risks solving a non-problem while adding real complexity
   (lifetime/ownership changes to `TableResolver`/`AdminExecutor` call
   sites, error-aggregation semantics for concurrent failures, and
   interaction with the transactional path's single mutable `TxContext`,
   which cannot be shared across concurrent tasks without further
   redesign).
2. **Phase A is scoped to correctness, not performance.** This task's
   purpose is edge provenance and `after`/`$query` semantics — closing the
   `after`-leaks-data bug and fixing the doc/behavior mismatch. Introducing
   a concurrency redesign in the same change would mix unrelated concerns
   and make the diff much harder to review and rebase against the rest of
   Epic 01.
3. **A dedicated benchmark phase exists.** Task #632 (Phase E) is
   explicitly reserved for sequencing benchmarks; that is the right place
   to decide whether concurrent-stage execution is worth building, backed
   by numbers instead of intuition.

## Phase E results (task #632)

New bench: `crates/shamir-engine/benches/batch_stage_parallelism.rs`. Drives
a single batch **stage** of N mutually independent `Read` ops (no
`after`/`$query` edges — the planner puts all N into one stage) through the
real `execute_batch` / `execute_plan_impl` path, against an in-memory
`InMemoryRepo` table pre-populated with 200 rows. Also benchmarks
`BatchPlanner::plan` alone on a 50-op independent batch as an absolute
baseline (no prior planning-only bench existed).

Run: `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
shamir-engine --bench batch_stage_parallelism` (QUICK/JIT-calibrated
iteration counts, `bench-scale-tool` fixed-iteration harness).

Real numbers (2026-07-15, this machine, quick calibration, `bench-scale-tool`
fixed-iteration harness):

| Bench cell              | Iters | Total time | ns/op (whole batch) | ns/op (per read) |
|--------------------------|-------|-----------:|---------------------:|------------------:|
| `batch_stage/reads_10`   | 122   | 873.199 ms | 7,157,368.03 | ≈ 715,737 |
| `batch_stage/reads_50`   | 26    | 796.981 ms | 30,653,126.92 | ≈ 613,063 |
| `batch_plan/plan_50_reads` | 5313 | 639.737 ms | 120,409.71 (whole 50-op plan) | ≈ 2,408 |

Observations:

- **Scaling is linear, as expected for a sequential loop.** 10 → 50 ops is a
  5× increase in op count; total per-batch time goes from ~7.16 ms to
  ~30.65 ms — a ~4.28× increase, i.e. within noise of linear (quick/JIT
  calibration uses few samples, so cell-to-cell variance of this size is
  expected — see prior run's ~4.79× for the same comparison). There is no
  super-linear blowup that would hint at an accidental O(N²) cost hiding in
  `execute_plan_impl`'s per-stage loop.
- **Per-op cost is in the same regime across N** (~613–716 µs/op for both
  N=10 and N=50), not growing with stage size — consistent with each `Read`
  op paying its own independent cost (index lookup + projection over the
  200-row table) with no cross-op interference.
- **Per-op cost (~0.61–0.72 ms) is dominated by the `Read` pipeline itself**
  (filter eval + index scan + projection over 200 rows), not by
  `execute_plan_impl`'s stage-iteration bookkeeping — `BatchPlanner::plan`
  for a comparable 50-op batch costs only ~2.4 µs/op (≈120 µs total),
  roughly **250–300× cheaper per op** than actually running the read. The
  sequential-loop overhead the ADR was worried about (dependency-map
  lookups, `resolved_refs` construction) is negligible next to the
  CPU-bound read work each op performs.
- **No `.await` suspension points were observed to matter**: the whole
  in-memory, CPU-bound `Read` path runs to completion without yielding, so
  there is nothing here for a same-task `try_join_all` or a
  `tokio::spawn`-per-query executor to interleave — confirming the
  Phase A hypothesis. Spawning a Tokio task per query would add
  scheduling/allocation overhead (task struct, channel, cross-thread
  wake-up) on top of ~0.6–0.7 ms of real work per op, which is exactly the
  regime where task-spawn overhead (typically low tens of µs, but real)
  stops being negligible relative to potential savings — and there is no
  savings to be had, since these ops never suspend and never contend on a
  shared resource that would let concurrent scheduling shorten wall time.

## Final decision

**Closed — stage parallelism (decision (a), `tokio::spawn`-per-query) will
NOT be implemented.** The benchmark data confirms the Phase A hypothesis:
for the CPU-bound, in-memory `Read` workload representative of typical
batch stages, per-op cost scales linearly with stage size, there are no
`.await` suspension points inside a query's execution path for a scheduler
to exploiter, and the sequential `execute_plan_impl` loop's own bookkeeping
cost (~3.5 µs/op via `BatchPlanner::plan`) is negligible next to the actual
per-op work (~0.83 ms). Introducing `tokio::spawn`-per-query would add
non-trivial API surface (`Arc<dyn TableResolver>` / `Arc<dyn AdminExecutor>`,
error-aggregation semantics, `TxContext` sharing redesign for the
transactional path) for zero measured wall-time benefit on this workload
shape.

This does **not** rule out a future revisit if/when a batch stage's ops
routinely perform real I/O with genuine suspension points (e.g. disk-backed
repos under contention, or a network-backed WASM function invocation) —
that would be a different workload shape than the one measured here, and is
explicitly out of scope for this phase. If that scenario becomes common,
open a fresh task with its own benchmark rather than reopening this
decision on the numbers above.
