# ADR: OQL Epic 03 / Phase A — Conditional execution (`when`/if/switch on batch ops)

Status: accepted (design only — no code in this change).
Task: #644. Roadmap: `docs/dev-artifacts/roadmap/oql/03-conditional-execution.md`.
Research: `docs/dev-artifacts/research/oql/04-conditionals-feasibility.md` §3-§6.

## Context

Today every alias declared in a `BatchRequest.queries` map is guaranteed to
execute exactly once (`BatchPlanner` builds a static DAG of ALL entries;
`execute_plan_impl`/`execute_plan_tx_impl` iterate every stage and every
alias in it unconditionally). Epic 02 added `$cond`/`$expr` as **value-level**
ternary/expression primitives (evaluated once per row, inside an
already-executing op). There is still no way to skip an entire op
(INSERT/UPDATE/DELETE/DDL/Call/sub-batch) based on a runtime value computed
earlier in the same batch — the existing `WHERE`-no-op emulation is leaky
(doesn't neutralize INSERT/Set/DDL/Call/Batch, pollutes the tx read-set,
double-scans) and WASM `Call` runs autocommit outside the batch's
transaction, so it cannot serve as a substitute for "run B only if A (in the
same tx) produced X".

This ADR fixes the four design decisions from roadmap Phase A / research §6,
so Phase B (#645) has a concrete contract to implement against.

---

## Decision 1 — Form of the primitive

**Decision: confirmed as drafted.** Add `when: Option<Filter>` to
`QueryEntry` (`crates/shamir-query-types/src/batch/query_entry.rs`, sibling
to the existing `op`/`return_result`/`after` fields). Semantics: the op
executes iff `when` is absent, or present and evaluates to `true` via the
same `Filter` evaluation machinery WHERE clauses already use
(`crate::query::filter::compile_filter` / `FilterNode::matches` in
`crates/shamir-engine/src/query/filter/`) — NOT a new evaluator.

Rationale for `Filter`, not `FilterValue`:
- `when` is a **boolean predicate over an op**, evaluated exactly once per
  op invocation — it is structurally identical to a WHERE clause on a
  virtual "should this op run" query, not a per-row computed value.
  `FilterValue` (and `$cond`/`$expr` within it) selects between two
  *values* inside an already-running, per-row op; `when` selects whether
  the *op itself* runs at all.
- Many op kinds that need conditional execution have **no per-row context
  to evaluate a value against**: `InsertOp` has no WHERE-eligible input
  rows before it writes them, DDL ops have no rows at all, `CallOp` invokes
  a function once (not per-row). A `Filter` evaluated against an *empty
  synthetic record* (no `FieldRef` support needed/meaningful — only
  `$query`/`$fn`/`$param`/literals matter for `when`) models this correctly;
  reusing `FilterValue`'s per-row semantics would be a category error for
  non-Read ops.
- Reuse is real, not just structural: `Filter` already has full serde,
  builder support (Rust + TS), depth/nesting limits
  (`validate_filter_depth`), and (per Decision on bug #642 below) the exact
  dependency-extraction path WHERE clauses use. No new wire type, no new
  parser branch, no new limit dimension.

Switch-case is confirmed as **builder-only sugar** (Phase C, #646), not a
new wire primitive: `b.switch(&handle).case(v1, op1).case(v2, op2).default(op)`
expands to N `QueryEntry`s with complementary `when` filters — `case1`,
`AND(NOT case1, case2)`, ..., `default = NOT any_case`. This keeps the
wire/engine diff to a single field, and the complementary-filter
construction is exactly the kind of mechanical `Filter::And`/`Filter::Not`
composition the builder layer already does elsewhere (e.g. row-filter AND
merge in `batch_execute.rs::and_combine`).

**Field placement note (non-normative, for #645):** `when` should sit next
to `after` in `QueryEntry`'s `#[serde(flatten)] op: BatchOp` sibling fields,
`#[serde(default, skip_serializing_if = "Option::is_none")]`, so it is
fully backward-compatible (omitted → today's unconditional-execution
behavior, byte-identical wire format for every existing client).

---

## Decision 2 — Semantics of a dependency on a skipped op

**Decision: confirmed as drafted**, with the `after`-edge carve-out made
fully explicit (it already has precedent in Epic01/A).

- **Cascading skip.** If alias `A` is skipped (`when` evaluated `false`),
  and alias `B` has a `DataFlow` or `Both`-provenance edge onto `A` (a real
  `$query`/`$ref` dependency on `A`'s result, or `A` appears in `B.when`
  itself), then `B` is also skipped — automatically, with status `skipped`,
  not an error. This matches ordinary if/else intuition: code inside a
  branch that wasn't taken (including everything that reads its output)
  simply doesn't run.
- **`Explicit`-only (`after`) edges do NOT cascade.** Epic01/A already
  established that `after` is pure ordering, not a data-access grant (see
  `crates/shamir-engine/src/query/batch/batch_execute.rs` doc comment:
  "NOT `after`-only (`Explicit`) dependencies, which are pure ordering and
  grant no data access" — `build_resolved_refs` already excludes
  `Explicit`-only deps from `resolved_refs`). Consistently, if `B after A`
  (pure ordering) and `A` is skipped, `B` still runs (assuming `B` has no
  own `when` and no `DataFlow`/`Both` edge onto `A`) — it simply runs
  without whatever ordering guarantee "after A" was providing, since there
  is nothing left to order against. This is a direct extension of an
  already-shipped invariant, not a new judgment call.
- **No separate `UnresolvedRef` error path.** Because `resolved_refs` is
  built per-alias from `plan.edge_provenance` (`build_resolved_refs(&all_results, deps)`),
  and a skipped alias never gets an entry in `all_results`, an attempt by a
  non-cascaded dependent to resolve `$query` against a skipped alias would,
  under today's code, silently resolve to `None` (`ctx.resolved_refs.get(key)?`
  in `resolve.rs:183` — ordinary Option `?`-propagation, same "absent"
  semantics `$param` already has). Phase B must make this impossible to
  observe as silent-None: any op with a genuine `DataFlow`/`Both` edge onto
  a skipped alias MUST be part of the cascade (computed transitively at
  execution time, stage-by-stage, before a skipped-dependent op is
  attempted) rather than being executed and silently seeing an absent
  value. This is a planning/execution contract addition for #645, not a
  runtime special case bolted onto `resolve_filter_query`.

---

## Decision 3 — Repo-scope / is_write / authorization

**Decision: confirmed as drafted.** Pessimistic (maximal-branch) model:
ALL declared ops — including those whose `when` may evaluate `false` at
runtime — participate in `distinct_repos`, `begin_tx` repo selection, and
`is_write` classification at **plan time**, as if they were guaranteed to
execute.

Rationale, verified against the actual code:
- `distinct_repos()` (`crates/shamir-query-types/src/batch/query_entry.rs:73-78`)
  iterates `queries.values()` unconditionally and calls `qe.op.table_ref()`
  — it has no visibility into `when` today, and Phase B must NOT teach it
  to filter by `when` (since `when` is a runtime value, unknown at the
  point authorization/repo-scope/cross-repo-guard decisions are made, all
  of which happen in `execute_batch_impl` BEFORE `execute_plan_impl`/`execute_plan_tx_impl`
  run — see `batch_execute.rs:76-83` cross-repo guard and
  `execute_transactional_impl:427-428` repo selection, both called ahead of
  any op executing).
- This is safe-by-default: better to reject a batch that COULD touch a
  write (and in a given call, its `when` happens to be `false`) than to
  authorize it as read-only and then discover at runtime that `when`
  evaluated `true` for a write op that was never checked.
- This is consistent with (not a new precedent, but the same rule as)
  `BatchOp::is_write` for `Batch(sub)` — "write if ANY nested op is write"
  regardless of whether that nested op will actually run
  (`docs/dev-artifacts/research/oql/01-nested-batch-recursion.md`, cited
  correctly by the brief). A read-only follower REJECTS a batch containing
  a condition-guarded write op, even in a call where `when` would evaluate
  `false` — this is intentionally conservative and must be documented as
  such (Phase G) so it isn't mistaken for a bug later.

No code changes needed for this decision beyond what #645 already touches
in the executor — `distinct_repos`/`is_write`/cross-repo-guard/authorization
call sites are correct as-is (they already scan the full static `queries`
map) and must NOT be modified to consult `when`.

---

## Decision 4 — Status in the response

**Decision: add `skipped: bool` field to `QueryResult`, not a new enum
variant.** Verified against `crates/shamir-query-types/src/read/query_result.rs`:
`QueryResult` is a plain struct (`records: Vec<QueryRecord>`, `stats: Option<QueryStats>`,
`pagination: Option<PaginationInfo>`, `value: Option<QueryValue>`,
`explain: Option<ExplainPlan>`), not an enum — every one of its other
"mode" distinctions (tabular vs scalar Call result, paginated vs not,
explain vs real execution) is already expressed as an additional `Option`
field on the same struct, never as an enum discriminant. Converting it to
an enum (`QueryResult::Skipped` vs `QueryResult::Executed{...}`) would force
every existing construction site and match arm across
`shamir-engine`/`shamir-db`/`shamir-client`/tests to add a new arm —
disproportionate blast radius for a status flag. A
`#[serde(default, skip_serializing_if = "std::ops::Not::not")]
skipped: bool` field (default `false`, omitted from wire when `false` — old
peers never see it) is the minimal, backward-compatible change:
- executed op → `skipped: false` (or field omitted), `records`/`value` as
  usual.
- skipped op (own `when` false, or cascaded) → `skipped: true`,
  `records: vec![]`, `value: None`, `stats: None`, `pagination: None`,
  `explain: None` — a recognizably-empty-but-explicitly-marked result,
  distinguishable both from "0 records matched" (`skipped: false`,
  `records: []`, `stats: Some(..)` with `records_scanned` etc.) and from
  "filtered out by `return_only`" (alias absent from `BatchResponse.results`
  entirely, per `filter_results` in `batch_execute.rs:543-558` — unchanged).

Confirmed as drafted: a skipped alias is still present in
`BatchResponse.execution_plan`/`edge_provenance` (both already echo the
full static plan — `execution_plan: std::mem::take(&mut plan.stages)`,
`edge_provenance: std::mem::take(&mut plan.edge_provenance)` in
`batch_execute.rs:169-170` — this needs no change, since stages/provenance
are static/plan-time artifacts, independent of runtime skip decisions).
Whether a skipped alias appears in `results` at all is governed by the
EXISTING `return_result`/`return_all`/`return_only` filtering
(`filter_results`), unchanged by this feature — i.e. "skipped" is a
property of an entry that IS present in `results`; "not present in
`results`" continues to mean exactly what it means today (filtered by
`return_result: false` / `return_only`), and does NOT gain a second, skip
-related meaning. This is the intended distinction the brief asked for:
"skipped" is client-visible (as a field on a present result); a `false`
`return_result` op that also happens to have `when: false` produces no
`results` entry at all — same as any other `return_result: false` op today
— so "invisible because filtered" and "invisible because return_result:
false" remain the SAME case (return_result already wins in that
combination, there is nothing new to reconcile).

---

## Bug #642 relevance to `when: Filter` — explicit finding

**Verified by reading the code: bug #642 DOES affect `when: Filter`. It is
NOT a separate, unaffected code path.** This contradicts the brief's
speculative hope in §"Учесть баг #642" that Filter-tree dependency
extraction might be a distinct path from `FilterValue`-tree extraction.

Evidence (`crates/shamir-query-types/src/batch/planner.rs`):

- `extract_deps_from_filter` (lines 283-340) is the Filter-tree walker used
  for WHERE clauses (`BatchOp::Read.r#where`, `Update.where_clause`,
  `Delete.where_clause`) today, and would be reused verbatim for
  `QueryEntry.when: Option<Filter>` in Phase B (there is no reason to write
  a second Filter-tree walker — the brief's brief itself assumes reuse, and
  Decision 1 above confirms `when` reuses `Filter`, the same grammar as
  WHERE).
- Every leaf arm of `extract_deps_from_filter` that carries a `FilterValue`
  (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`Contains` at line 291, `In`/`NotIn` at
  296, `Between` at 300-301, `ContainsAny`/`ContainsAll` at 305, `FieldEq`
  at 317, `Computed`'s `value`/`expr_args` at 332-336) delegates to
  `Self::extract_deps_from_filter_value(value, deps)` — **the exact same
  function** bug #642 lives in.
- `extract_deps_from_filter_value` (lines 343-358) is:
  ```rust
  match value {
      FilterValue::Array(arr) => { /* recurse */ }
      FilterValue::QueryRef { alias, .. } => { /* record dep */ }
      _ => {}   // <-- Cond, Expr, FnCall, Param, and all literals fall here
  }
  ```
  `FilterValue::Cond`, `FilterValue::Expr`, and `FilterValue::FnCall` all
  fall into the `_ => {}` catch-all and contribute NO dependency, even
  though each can syntactically contain a nested `FilterValue::QueryRef`
  (a `$query` reference inside a `$cond`'s `then`/`or_else`, inside an
  `$expr`'s `args`, or inside a `FnCall`'s `args`).
- Therefore: if `when` is written as, e.g.,
  `Filter::Eq { field: [...], value: FilterValue::Cond { cond: Box::new(Cond { condition, then: FilterValue::QueryRef{ alias: "check", .. }, or_else: FilterValue::Bool(false) }) } }`
  — a `when` that decides its outcome via a `$cond` referencing another
  alias's result — the planner will silently fail to record `check` as a
  dependency of the entry owning `when`. The DAG/topological sort would not
  guarantee `check` runs before the `when`-guarded entry, and (worse) if
  `check` itself is ever skipped, no cascade would be triggered for the
  `when`-guarded entry, because the planner never knew about the edge in
  the first place. This is a silent-wrong-order / silent-non-cascade bug,
  not a crash — the most dangerous kind for this feature.
- There is no evidence of a second, `Filter`-specific `$query`-ref
  extractor anywhere in `shamir-query-types::batch` or elsewhere in the
  workspace; `extract_deps_from_filter_value` is the sole leaf-level
  extractor for both WHERE-Filter and (prospectively) `when`-Filter trees.

**Consequence for Phase B (#645):** `when` dependency extraction is
correct TODAY only for `when` filters that reference other aliases
directly at the `FilterValue::QueryRef` leaf (e.g.
`Filter::Eq { value: FilterValue::QueryRef{..} }`, `Filter::In { values: [FilterValue::QueryRef{..}] }`)
— the common, expected case for a simple `if` condition. It is INCORRECT
(silently drops the dependency) for `when` filters that reach a `$query`
ref through a nested `$cond`/`$expr`/`FnCall` — i.e. exactly the same class
of `when` expressions that would make heavy use of Epic02's newly-added
value-level conditionals to compute the boolean gate itself. Given that
`when` is expected to be a natural consumer of `$cond`/`$expr` composition
(a boolean gate is a prime use case for exactly those constructs), **Phase
B (#645) should not proceed on `when` dependency extraction independently
of #642** — either #642 is fixed first (making
`extract_deps_from_filter_value` recurse into `Cond.condition`/`Cond.then`/
`Cond.or_else`, `Expr.args`, and `FnCall`'s args), or #645 fixes the
recursion itself as part of implementing `when` (in which case the fix
should live in the shared `extract_deps_from_filter_value` so WHERE clauses
benefit too, not in a `when`-only special case). Recommendation: block
#645's dependency-extraction sub-task on #642, or explicitly fold #642's
fix into #645's implementation PR — do not ship `when` support that only
handles the "no nested `$cond`/`$expr`/`FnCall`" subset without a tracked
follow-up, since that would reproduce the exact silent-wrong-dependency
class of bug the roadmap is trying to avoid for a brand-new feature.

---

## Summary of decisions

| # | Decision | Confidence |
|---|---|---|
| 1 | `when: Option<Filter>` on `QueryEntry`; switch-case is builder sugar generating complementary `when`s, not a wire primitive | High — confirmed, no contradicting code found |
| 2 | Cascading skip via `DataFlow`/`Both` edges; `Explicit`(`after`)-only edges do not cascade; no separate `UnresolvedRef` error path — cascade must be computed transitively before execution, not discovered as silent-None at eval time | High — confirmed, direct extension of Epic01/A's existing `after`-is-ordering-only invariant |
| 3 | Pessimistic authorization/repo-scope/is_write over the full static op set, independent of runtime `when` | High — confirmed, consistent with existing `Batch(sub)` is_write precedent and the existing call-order (auth happens before any op runs) |
| 4 | `skipped: bool` field added to `QueryResult` (not a new enum variant); alias stays in `execution_plan`/`edge_provenance`; presence in `results` still governed solely by existing `return_result`/`return_all`/`return_only` | High — confirmed by reading `QueryResult`'s actual struct shape (already field-based, never enum-based, for every other status dimension) |
| #642 relevance | **`when: Filter` IS affected by bug #642** — same `extract_deps_from_filter_value` leaf extractor as WHERE clauses, same `_ => {}` catch-all swallowing `Cond`/`Expr`/`FnCall` | High — confirmed by direct code read, brief's "maybe separate path" hope is refuted |

No open questions requiring orchestrator judgment calls — all four
decisions confirm the brief's draft recommendations as architecturally
sound given the actual code, and the #642 investigation produced a
definitive (not speculative) answer.
