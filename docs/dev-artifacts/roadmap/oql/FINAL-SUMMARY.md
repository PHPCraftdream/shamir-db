# OQL Roadmap — Final Summary Across All 4 Epics

Covers the full 2026-07 OQL campaign: Epic01 (Sequencing explicitness),
Epic02 ($cond/$expr value evaluation), Epic03 (Conditional execution —
`when`/`switch`), Epic04 (Loops — `for_each`). Task numbers and commit
hashes below were checked against `git log --oneline` and the live
TaskList at time of writing (2026-07-16); anywhere a specific hash could
not be independently confirmed, this document says so explicitly rather
than inventing one.

This is not a victory-lap document. Three of the four epics shipped with
honestly-documented gaps, and one of them (#651) is CRITICAL — it silently
breaks the exact canonical scenario that epic's own ADR used to justify
building the feature.

## 1. What was built, epic by epic

**Epic01 — Sequencing explicitness** (tasks #628-633, roadmap doc
`01-sequencing-explicitness.md`). Made `after`-edges and `$query`-driven
data-flow edges distinct and explicit in the batch DAG (`EdgeKind`
provenance: `After` vs `DataFlow` vs `Both`), removing the prior implicit
coupling where `after` alone could accidentally imply data access.
Engine: `b65b4940` (edge provenance, `after` no longer opens data).
Builders: `72dca050` (Rust fluent `.after()`, TS `build()` validation).
Unit tests: `851bf79e`. E2E (Rust+TS, real wire): `fddd0efd`. Benchmarks
(stage parallelism decision): `b0656dee`. Docs + closure: `05644e2f`.
Shipped clean, no known open bugs from this epic.

**Epic02 — `$cond`/`$expr` value evaluation** (tasks #635-640, roadmap doc
`02-cond-value-evaluation.md`). Added conditional VALUE selection inside
an already-executing query (`$cond`/`switch_case` picks between two
values, evaluated against `resolve_filter_query`) — distinct from Epic03's
`when`, which gates whether an entire operation runs. Engine: `cdcfc0f3`.
Builders (`switch_case` sugar): `28f265f7`. Unit tests: `6ebaa8c3`. E2E,
scoped to WHERE-position only (not SET, see #641 below): `9dd1a28c`.
Benchmarks: `9e084e54`. Docs + honest known-limitations: `d85c1cc2`.
Also fixed along the way: `BatchPlanner` failing to recurse into
`$cond`/`$expr`/`FnCall` when extracting dependencies (task #642,
resolved inside Epic03/B per `997532cc`'s commit message — filed during
Epic02, fixed as a side-effect of Epic03's engine work).

**Epic03 — Conditional execution (`when`/`switch` on ops)** (tasks
#644-650, roadmap doc `03-conditional-execution.md`). Added `when: Filter`
on a `QueryEntry` — the whole operation (INSERT/UPDATE/DELETE/DDL/Call/
sub-batch) executes only if `when` evaluates true, with cascading skip for
`DataFlow`/`Both`-dependent aliases, pessimistic (worst-case)
authorization regardless of runtime skip decisions, and an explicit
`skipped` status field in the response distinct from "0 records" and from
`returnResult: false` filtering. ADR: `7a4d252e`. Engine (`QueryEntry.when`,
cascading skip, also fixed #642): `997532cc`. Builders (`when`/`switchCase`
helpers): `1dd6e57c`. Unit tests: `5be739f5`. E2E: `1d20ae60`. Benchmarks
(skip vs. full-execution cost): `054e6676`. Docs + honest closure
(prominently documenting CRITICAL bug #651 rather than hiding it):
`6ab3a573`. **This epic's core promised use case does not work today** —
see #651 below.

**Epic04 — Loops (`for_each`)** (tasks #652-658, roadmap doc
`04-loops-foreach.md`). Added `BatchOp::ForEach` — `over` (a `$query`
column-ref, `$fn` call, or literal array) resolves to a list exactly once,
then a sub-batch body executes once per element with the element bound to
a named parameter (`bind_row`), all within one transaction where
applicable. ADR: `e267406b`. Engine (`ForEachOp`, planner, K-fold
executor): `6ff521d5`. Builders (Rust `b.for_each(...)`, TS
`b.forEach(...)`): `79510a13`. Unit tests: `7ed75075`. E2E (real
`$query`-driven canonical scenario, real wire): `f0ccf786`. Benchmarks:
`a565d436`. Docs + closure: this phase (#658). **Unlike Epic03, this
epic's canonical use case works as designed** — confirmed by a real e2e
test iterating over genuine `$query` results, no synthetic workaround
needed.

## 2. What works fully, with no known limitations

- **Epic01 sequencing** — `after`/`$query` DAG edges, `EdgeKind`
  provenance (`After`/`DataFlow`/`Both`). No open bugs from this epic at
  time of writing.
- **Epic04's `for_each` canonical scenario** — genuine `$query`-driven
  iteration (read rows, loop-insert one row per element, transactional),
  confirmed end-to-end over a real wire connection in
  `crates/shamir-client/tests/batch_for_each_e2e.rs`. No known correctness
  bug analogous to #651.

## 3. What works with known limitations

- **#641 — GAP**: `$cond`/`FilterValue` does not compose into write
  SET-values (a `QueryValue` vs `FilterValue` type split in the current
  model). Not a bug in the strict sense — a real design gap that needs its
  own scoped design/ADR before it can be closed. Epic02's e2e coverage was
  deliberately scoped to WHERE-position only because of this gap.
- **#643 — PERF**: `$cond`/`$expr` evaluation recompiles the filter on
  every row (roughly 29-190x overhead vs. an equivalent flat literal, per
  Epic02's benchmark). Correctness is unaffected; this is purely a
  throughput cost today.
- **#651 — CRITICAL, still open.** Field-based `when` comparisons
  (`Eq`/`Gt`/`Gte`/`Lt`/`Lte`/`Ne`/`FieldEq`) inside `resolve_skip` always
  collapse to a fixed result, because they compile against an empty
  synthetic record through a one-shot scratch interner that can never
  resolve a real field path for any field name. In practice this means
  **every field-comparison `when` silently gives the same answer
  regardless of real data** — including Epic03's own ADR's canonical
  motivating example ("debit the account if balance >= amount"), which is
  silently broken, not merely limited. This is the single most impactful
  open issue from the entire 4-epic campaign: it undermines Epic03's core
  promised capability. The only reliable `when` pattern today is a
  presence guard (`isNull`/`isNotNull` against a field known to be
  absent), which the shipped docs document as the sole safe usage.
- **#660 — FIXED (2026-07).** `distinct_repos()` now walks recursively
  into `Batch`/`ForEach` bodies, so a transactional batch whose ONLY
  top-level entry is a bare `Batch`/`ForEach` correctly determines its
  repo scope (and nested cross-repo bodies are visible to the cross-repo
  guard). `BatchOp::table_ref()` intentionally still returns `None` for
  both variants — the recursion lives in `distinct_repos`'s collector.
- **Benchmark-derived characteristic (not a bug)**: `ForEach` carries real
  per-iteration overhead vs. a hand-written flat batch — roughly
  1.5-1.6x slower for a one-op body, growing slightly with N rather than
  amortizing to a fixed cost (Epic04/Phase F finding, independently
  re-run and confirmed). This is a genuine design trade-off — ergonomics
  and data-dependence over raw throughput — not a defect, and is stated
  plainly in both the guide docs and here so users choose `for_each`
  deliberately rather than by default when N is fixed and known upfront.

## 4. Explicitly deferred / not started

- **#659 — while-style loops** (per-step condition re-evaluation, as
  opposed to `for_each`'s resolve-once-then-iterate shape). This is a
  genuinely new primitive shape — not a variant of `for_each` — proposed
  by the user mid-session and deliberately deferred to keep Epic04 scoped
  to the simpler, already-substantial for-each shape. No design work has
  started on it.

## 5. Process learnings worth preserving

- **Concurrent sub-agent corruption risk is real, not theoretical.**
  During Epic04/Phase B, two agent continuations of the same
  in-flight task ran simultaneously and edited the same files, producing
  a corrupted intermediate state. It was caught by the user before it was
  committed and salvaged through read-only diagnosis (comparing the two
  divergent edit sets against the ADR) rather than discarding either
  agent's work outright. Lesson: never resume/re-launch a continuation of
  a task that may still have another live agent attached to it — verify
  single-ownership before resuming.
- **Growing a struct/enum keeps breaking construction sites in OTHER,
  untouched crates.** This recurred enough times across the 4 epics
  (adding fields to `QueryEntry`, `BatchOp` variants, `BatchLimits`) that
  workspace-wide `cargo clippy --all-targets -- -D warnings` became
  mandatory after every phase — not just clippy scoped to the crates a
  phase's brief listed as in-scope. A crate-scoped clippy run would have
  missed several of these breaks.
- **Honest-limitations documentation caught a critical bug before it
  reached "epic done" status uncritically.** Epic03/G's brief explicitly
  required a prominent warning block for #651 rather than a clean feature
  writeup; writing that warning is what forced the team to state plainly
  that the epic's own canonical use case doesn't work. Epics that skip a
  deliberately adversarial "what doesn't work" pass in their closing docs
  phase risk shipping documentation that oversells the feature.
- **Benchmark-first honesty prevented an ergonomics feature from being
  sold as a performance feature.** Both Epic02 (#643) and Epic04 (Phase F
  finding) needed their bench doc-comments to state plainly that the new
  primitive is slower than the hand-written alternative — the value
  proposition is expressiveness/data-dependence, not raw throughput. This
  needed explicit framing in the guide docs to avoid users reaching for
  `for_each`/`$cond` by default where a flat batch would serve better.

## 6. Recommended next steps

1. **Fix #651 first.** It blocks Epic03's actual promised value (data-driven
   conditional execution); today `when` is only a reliable
   presence-guard/feature-flag mechanism, not the general
   conditional-execution-on-data primitive the roadmap set out to build.
2. **Fix #660 next — DONE (2026-07).** Small and mechanical — extend
   `distinct_repos()`/`table_ref()` to walk into `Batch`/`ForEach` bodies;
   low risk, closes a real but narrow gap affecting both Epic01 and
   Epic04 constructs.
3. **Scope #641 and #643 as their own dedicated efforts** — #641 needs a
   real design decision (how `QueryValue` and `FilterValue` should unify
   or interoperate for write SET-values) before implementation; #643 is a
   pure performance investigation (likely caching compiled filters keyed
   by their AST shape) that can proceed independently once #641's design
   direction is settled.
4. **Revisit #659 (while-style loops) as a fresh design exercise** only if
   there is still a real product need for per-step re-evaluated
   conditions after #651 is fixed — with `when` actually working
   correctly, some of the motivating use cases for `while` may be
   achievable via repeated `for_each` + `when` composition instead of a
   wholly new primitive.
