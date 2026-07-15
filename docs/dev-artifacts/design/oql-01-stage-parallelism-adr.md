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
