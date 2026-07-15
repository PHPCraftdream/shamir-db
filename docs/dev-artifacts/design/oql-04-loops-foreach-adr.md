# ADR: OQL Epic 04 / Phase A — Loops (`ForEachOp`)

Status: accepted (design only — no code in this change).
Task: #652. Roadmap: `docs/dev-artifacts/roadmap/oql/04-loops-foreach.md`.
Research: `docs/dev-artifacts/research/oql/03-loops-feasibility.md`.

## Context

Epic01 shipped `BatchOp::Batch(SubBatchOp)` — a nested `BatchRequest` with
explicit `bind: TMap<String, FilterValue>` parameter injection
(`crates/shamir-query-types/src/batch/sub_batch_op.rs`). Epic02 added
`$cond`/`$expr` value-level evaluation. Epic03 added `QueryEntry.when` +
cascaded skip. None of these let a batch execute a body **K times**, once
per row of a prior result, inside one atomic scope — the research report
(§5-6) confirms this is the one remaining gap after set-based DML,
`@alias[].field` column-refs, and WASM `Call` are accounted for: an atomic,
data-dependent fan-out within a single batch/transaction.

This ADR fixes the five design decisions from roadmap Phase A, verified
against the real `SubBatchOp`/`QueryRunner`/`BatchPlanner`/`BatchLimits`
code (not merely the roadmap's prose), so Phase B (#653) has a concrete
contract to implement against. It also settles the roadmap's explicit
question about bug #651's blast radius on the new primitive's `bind_row`
mechanism.

---

## Decision 1 — Form of the primitive

**Decision: confirmed as drafted.** Add

```rust
pub struct ForEachOp {
    pub over: FilterValue,
    pub bind_row: String,
    pub batch: BatchRequest,
}
```

as `BatchOp::ForEach(ForEachOp)` (wire key `for_each`), structurally a sibling
of `SubBatchOp` (`crates/shamir-query-types/src/batch/sub_batch_op.rs`):
`batch: BatchRequest` is reused verbatim; `bind: TMap<String, FilterValue>`
is replaced by the pair `over: FilterValue` (iteration source) +
`bind_row: String` (the name under which the current element is exposed to
the body, resolved the same way `SubBatchOp.bind`'s values are resolved
today). The body is planned **once** (as a static `BatchRequest`, exactly
like a sub-batch today) and executed **K times**, each iteration re-running
`execute_batch_impl` on the same static body with `resolved_params =
{ bind_row: element_i }` swapped in per iteration — the identical recursive
seam `QueryRunner::run`'s `BatchOp::Batch` arm already uses
(`crates/shamir-engine/src/query/batch/query_runner.rs:209-308`), just
invoked in a loop instead of once.

`over` is deliberately `FilterValue`, not `Filter`, confirming the brief's
recommendation:
- It is a **value-producing** expression (the vector to iterate), not a
  boolean predicate — the same class of thing `SubBatchOp.bind`'s map
  values already are (`FilterValue`, resolved once via
  `resolve_filter_query` against a scratch record, see Decision on #651
  below). `Filter` (used for WHERE/`when`) answers "does this row match a
  predicate"; `over` answers "what is the list of elements to iterate",
  which is exactly what `FilterValue::QueryRef` (`@alias[].field` — the
  existing "whole column" mechanism cited by the research report §2/§4,
  `resolve_query_ref_column`) already returns: `Vec<QueryValue>`.
- The typical `over` value is `@alias[].field` (a query-ref column) or a
  literal `FilterValue::Array` (the degenerate "repeat N" case — an
  N-element list of don't-care/identical values, exactly as the brief
  states). Both are `FilterValue` variants today; no new value grammar is
  needed, only a resolution site that expects the result to be a *list*
  (not a scalar) and iterates it, which is a new *consumer*, not a new
  *producer* type.
- "repeat N" needs no separate wire shape: it degrades to `over:
  FilterValue::Array([FilterValue::Null; N])` (or any N-length literal
  array) with a `bind_row` the body simply never references. No dedicated
  `repeat: N` field is introduced — keeping the wire surface to one new op
  variant, one new struct.

`while`-loops remain explicitly out of scope (confirmed, matching roadmap
§Фаза A point 1): a declarative primitive cannot express an unbounded,
data-dependent exit condition without becoming a general-purpose
interpreter; that expressiveness class is already covered by the WASM
`Call` escape hatch documented in the research report §3, and duplicating
it declaratively is not this epic's goal.

---

## Decision 2 — Representation of iteration results

**Decision: confirmed as drafted, with the addressing extension deferred.**
The loop alias's `QueryResult.value` is `QueryValue::List(Vec<QueryValue>)`
— one entry per iteration, each entry itself a `QueryValue::Map` of the
body's own inner aliases → their results, exactly the shape a single
sub-batch already produces today (`query_runner.rs:286-307`: the inner
`BatchResponse.results` map round-trips through msgpack into a
`QueryValue::Map`). For `ForEach`, that same per-iteration map is collected
into a `List` instead of being returned bare — i.e. the loop's `value` is
literally `QueryValue::List(vec![sub_batch_1_value, sub_batch_2_value, ...])`
using the exact per-iteration value the existing sub-batch code path already
constructs, called K times instead of once.

External addressing `@loop[i].inner_alias[j].field` — **deferred to a
follow-up, not implemented in Phase B.** Rationale:
- `resolve_query_ref_value`'s path parser (research report §2) already
  supports `@alias[N].field` where `alias` resolves to `qr.value`
  (Call/sub-batch path) and `[N]` indexes are literal — this is the
  existing single-sub-batch addressing mode
  (`@sub.alias_name[0].id` per `query_runner.rs`'s doc comment at line
  292-294). Extending it one more level (`@loop[i].alias[j].field`, an
  index into the OUTER `List` before descending into the per-iteration
  map) is a mechanical extension of the same parser, not a new mechanism —
  but it is still new parser surface, new tests, and a new failure mode
  (out-of-range `i` against a runtime-only-known K) that Phase B does not
  need to ship to deliver the core value proposition (atomic K-times
  execution within one tx).
- The `@loop[].inner_alias` "whole column across iterations" form (project
  `inner_alias` out of every element of the `List`, mirroring
  `resolve_query_ref_column`'s existing "whole column across rows"
  semantics for `Read` results) is a natural companion and cheaper to
  reason about (no per-iteration index, no OOB case) — but it is *also*
  new parser surface over a `List`-of-maps shape that
  `resolve_query_ref_column` today does not walk (it walks `QueryRecord`
  rows, not a `QueryValue::List`). Recommend it as the **first** addressing
  extension to add in a follow-up task, ahead of indexed `[i]` access,
  precisely because it has no partial/OOB semantics to design.
- Until either extension lands, a caller who needs per-iteration
  cross-referencing from a sibling op must consume the whole `List` via
  `$query @loop` (the value as-is) and destructure client-side, or express
  the follow-up work as further `ForEach`/`Batch` bodies that take `over:
  {"$query": "@loop"}`. This is a real (documented) capability gap, not
  silently swallowed — Phase G's docs must state it as a known limitation
  with the same explicitness Epic01 gave `@alias[N]` literal-only indexing.

This point carries real uncertainty flagged for the orchestrator: whether
"good enough for v1" is `List`-only (my recommendation, ships fastest,
extends later without a breaking wire change since `List` already contains
everything an indexed accessor would need) or whether Phase B should bundle
the `[]`-projection form immediately because it is cheap. I recommend
deferring both, since the roadmap itself frames this as "reshить, нужна ли,
или только агрегат" (an open question), and the whole-`List` value is a
strict superset that a later addressing feature can be built on without
touching the wire format again.

---

## Decision 3 — Limits

**Decision: confirmed as drafted**, with the dynamic-`over` gating
resolved as recommended.

Add `max_iterations: usize` to `BatchLimits`
(`crates/shamir-query-types/src/batch/batch_limits.rs`), default **1000**.
Scale check against existing defaults: `max_queries: 50` (total ops per
batch, static), `max_dependency_depth: 10`, `max_nesting_depth: 4`. 1000 is
two orders of magnitude above `max_queries`'s default — deliberately so,
since `max_iterations` bounds a *different* axis (repetition count of an
already-limited body) and the real DoS backstop is the **product**
`iterations × body.queries.len()`, not `max_iterations` alone. A default
in the same order of magnitude as `max_queries` (e.g. 50) would make
`ForEach` nearly useless for its core use case (research report's own
example: "100 orders → 100 audit inserts"); 1000 is generous enough for
realistic fan-out sizes while still being a hard, finite ceiling, not
"unbounded".

**Static case (`over` is a literal array):** `iterations_upper_bound` is
known at plan time (`FilterValue::Array(arr).len()`). The planner (Phase B,
`BatchPlanner`) computes `iterations_upper_bound × body.queries.len()` and
folds it into the SAME budget check `max_queries` already enforces
(`crates/shamir-query-types/src/batch/planner.rs:110-115`,
`TooManyQueries` error) — i.e. a `ForEach` node contributes
`iterations × body.len()` "virtual op units" to the parent's total, not
just its own single node. This is required so `max_queries` cannot be
circumvented by wrapping a large body in a small-looking `ForEach` node
(exactly the DoS concern the roadmap raises: "циклом нельзя обойти
DoS-лимиты").

**Dynamic case (`over` is a `$query`-column-ref, e.g. `@alias[].field`):**
confirmed, this is NOT known at plan time — the real row count of `@alias`
is only known once `@alias` has actually executed, which happens at
runtime, stage-by-stage, exactly as the research report states (§5 point
2). The only practical gate in this case is a **runtime check immediately
before the first iteration**: once `over` resolves to a concrete
`Vec<QueryValue>` of length `actual_len`, compare `actual_len >
max_iterations` and, if exceeded, fail the `ForEach` node with a new
`BatchError` variant (e.g. `TooManyIterations { alias, actual: actual_len,
max: limits.max_iterations }`) **before executing iteration 0** — not
mid-loop, not as a soft truncation. This is the only option because:
- The planner (`shamir-query-types`, a pure-DTO crate with no I/O) cannot
  see live data by construction (per the research report §5 point 2: "Данные
  недоступны планировщику в принципе").
  So a plan-time reject for the dynamic case is architecturally impossible
  without giving the planner an execution capability it explicitly does
  not have.
- Truncating silently (running only the first `max_iterations` elements
  and dropping the rest) would silently change query semantics based on an
  invisible limit — unacceptable for a data-correctness-sensitive
  operation (an audit-log fan-out that silently only processes 1000 of
  5000 orders is a correctness bug, not a resource-limit nicety).
  A hard error surfaces the limit to the caller instead of a silent partial
  effect.
- The check must run before iteration 0 (not interleaved as
  "abort at iteration 1001") specifically because Decision 4 makes a
  mid-tx failure an abort of the WHOLE containing batch anyway — running
  1000 iterations of real work and then aborting on iteration 1001 wastes
  the work AND still produces the same "big error, no partial commit"
  outcome, but slower and with more contended lock time. Front-loading the
  length check costs one extra `Vec::len()` comparison and turns a
  wasted-work abort into an immediate reject.

This mirrors exactly how Epic03 treats a structurally-unknowable-until-
runtime quantity (`when`'s value) — a plan-time-conservative check plus a
runtime backstop, not a plan-time hole.

---

## Decision 4 — Error semantics at iteration `i` of `K`

**Decision: confirmed as drafted for the tx case; `stop-at-first` chosen
for the non-tx case, NOT collect-errors.**

**Inside a transactional batch:** an error at any iteration aborts the
WHOLE containing batch — consistent with existing tx-batch semantics.
Verified against `execute_plan_tx_impl`
(`crates/shamir-engine/src/query/batch/batch_execute.rs:367+`) and its
non-tx sibling `execute_plan_impl` (lines 278-330): both use a plain `for`
loop over `plan.stages` with `execute_single_impl(...).await?` — the `?`
propagates the FIRST error out of the whole function immediately, there is
no error-collection or continue-on-error logic anywhere in either path
today. `ForEach`'s per-iteration recursive call into `execute_batch_impl`
inherits this identically: iteration `i`'s failure propagates via the same
`?`-based short-circuit the recursive sub-batch call already uses
(`query_runner.rs:269-284`, `.map_err(...)?` around `execute_batch_impl`).
Inside an explicit `TxContext`, this is additionally required for
correctness, not just consistency: a partial set of committed writes from
iterations `0..i` with iteration `i` failing would leave the transaction in
an inconsistent intermediate state relative to the "atomic fan-out" value
proposition that is `ForEach`'s entire reason to exist (per the roadmap:
"весь цикл атомарен" is explicitly the win over the WASM per-iteration
autocommit path). Silently committing a partial prefix of iterations would
reproduce the exact non-atomicity `ForEach` was built to fix.

**Outside a transaction (autocommit batch):** **stop-at-first**, not
collect-errors. Verified against `execute_plan_impl`
(same file, lines 278-330) and `execute_batch_impl`
(lines 62+, calls whichever of `execute_plan_impl`/`execute_plan_tx_impl`
is appropriate) — today's autocommit batch, when it contains multiple
independent top-level ops, ALREADY stops at the first error: the `for
stage in &plan.stages { for alias in stage { ... .await? } }` loop's `?`
aborts the entire `execute_plan_impl` call (and therefore the entire
`BatchResponse`) on the first failing alias in a stage, even for aliases in
the SAME stage that have no dependency relationship to the failing one and
would otherwise be safe to keep running. There is no per-alias
try/collect-into-Vec<Result> anywhere in this function. `ForEach` iterations
are not distinguishable from independent top-level ops in this respect —
each iteration is, from the executor's point of view, just another
`execute_batch_impl` recursive call — so making `ForEach` behave as
collect-errors (continue all K iterations, return a `Vec<Result>`) would be
a NEW behavior inconsistent with how the rest of autocommit batch execution
already works, not a neutral choice between two equally-supported options.
Consistency with the existing (if perhaps surprising) "any failure aborts
the whole autocommit batch" behavior is the deciding factor: introducing
collect-errors ONLY for `ForEach` would create two different failure
philosophies inside the same feature area (independent top-level ops:
stop-at-first; loop iterations: collect-errors), which is a worse
inconsistency than the loop being "as strict as everything else already
is". If collect-errors semantics are wanted for bulk fan-out in the future,
they should be proposed as a batch-wide (not `ForEach`-only) execution mode
change, out of scope for this epic.

Partial results already produced by completed iterations before the failing
one are discarded along with the rest of the aborted autocommit batch's
results (same as today: a failing alias's error propagates out of
`execute_batch_impl` before `BatchResponse` is even constructed — there is
no partial-`BatchResponse`-on-error path today for the top-level case
either, so `ForEach` introduces no new partial-response contract).

---

## Decision 5 — Authorization

**Decision: confirmed as drafted.** `ForEachOp.batch`'s body is authorized
as a **template**, exactly once, at the parent batch's planning time — the
same pessimistic (maximal-branch) model Epic03 already established for
`when` (`docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`
Decision 3) and that `SubBatchOp` already implements structurally today.

Verified against the actual code:
- `BatchOp::is_write` for `Batch(sub)` is
  `sub.batch.queries.values().any(|qe| qe.op.is_write())`
  (`crates/shamir-query-types/src/batch/batch_op.rs:752`) — "write if ANY
  op in the body is a write", evaluated over the FULL static body,
  independent of how many times (if any at all — including zero, for an
  empty `over`) that body will actually run. `ForEach` must implement
  `is_write` identically:
  `BatchOp::ForEach(fe) => fe.batch.queries.values().any(|qe| qe.op.is_write())`
  — the iteration count (0, 1, or K, only known at runtime for a dynamic
  `over`) plays NO role in this classification, matching the sub-batch
  precedent exactly, including its most extreme edge case: a `ForEach`
  whose `over` happens to resolve to an EMPTY list at runtime (zero
  iterations) is still classified `is_write() == true` at plan time if its
  body contains a write op, for the same reason a sub-batch is a write even
  though sub-batches are never conditionally skipped today — the
  classification runs before any row is fetched, so it cannot know the
  runtime cardinality.
- `distinct_repos()`/cross-repo-guard/authorization all run in
  `execute_batch_impl` BEFORE `execute_plan_impl`/`execute_plan_tx_impl`
  (per the Epic03 ADR's Decision 3, re-confirmed here by re-reading the
  same call sites) — i.e. before ANY op, including a `ForEach` node's body,
  has executed even once. A `ForEach` node's body's `table_ref()`s must
  therefore be visible to `distinct_repos()` the same way `Batch(sub)`'s
  are today (walking into `sub.batch.queries` — `ForEach` needs the
  equivalent walk into `fe.batch.queries`), so the repo-scope/tx-open
  decision made before execution starts already accounts for every table
  the loop body could touch, regardless of how many iterations run.
- This is intentionally conservative in the same documented way as
  Epic03's `when`: a read-only follower/permission set REJECTS a batch
  containing a write-body `ForEach`, even for a call where `over` happens
  to resolve to zero elements at runtime (no iterations, no actual write
  performed) — this must be documented in Phase G alongside the equivalent
  `when` caveat, not treated as a newly-discovered inconsistency later.

No new authorization mechanism is introduced. `ForEach`'s `required_access`/
`is_write`/`table_ref`-walking implementations are a mechanical structural
copy of `SubBatchOp`'s existing recursive implementations, substituting
`fe.batch` for `sub.batch`.

---

## Bug #651 — independence of `bind_row` (confirmed)

**Verified by reading the code: `bind_row` (the `ForEach` analogue of
`SubBatchOp.bind`) is NOT affected by bug #651.** This confirms the brief's
expected conclusion via direct code reading, not by assumption.

Bug #651 lives in `resolve_skip`
(`crates/shamir-engine/src/query/batch/query_runner.rs:102-141`), which
evaluates `QueryEntry.when: Option<Filter>` — a **boolean predicate**
compiled via `crate::query::filter::compile_filter(filter, &scratch)` and
matched via `node.matches(&InnerValue::Null, &ctx)` against a **scratch
`Interner::new()`** instance created fresh on every call (line 135). Any
`Filter::Eq`/`Gt`/`Gte`/etc. leaf that references an actual field path
resolves that path against the scratch interner's (empty) symbol table,
which cannot possibly contain the real table's field-name → id mappings —
so any field-based comparison inside `when` structurally cannot resolve and
folds to `false`/`True` incorrectly. This is a `Filter`-tree /
`compile_filter` / `FilterNode::matches` bug specifically.

The `bind`-resolution path used by BOTH `SubBatchOp.bind` today
(`query_runner.rs:227-266`, the `BatchOp::Batch(sub)` arm) and, by direct
structural analogy, `ForEachOp.bind_row` (Phase B) is a COMPLETELY
DIFFERENT code path:
- It ALSO constructs a scratch `Interner::new()` (line 230) — so on the
  surface it "looks like" the same suspicious pattern — but it never calls
  `compile_filter`/`FilterNode::matches` at all. Instead, each `bind` map
  value (a `FilterValue`, not a `Filter`) is resolved via
  `crate::query::filter::eval::resolve_filter_query(other, &dummy_record,
  &bind_ctx)` (line 250-254) — a **direct value resolver**, not a filter
  predicate compiler/matcher.
- `resolve_filter_query` operates on `FilterValue` leaves directly:
  `FilterValue::Param { name }` looks up the outer scope's `self.params`
  map (lines 237-248, a plain `TMap` lookup — no interner involved at all
  for this branch); every other `FilterValue` (literal, `$query` ref, etc.)
  is resolved by `resolve_filter_query` against the `dummy_record =
  InnerValue::Null` and the scratch-interner-backed `bind_ctx`. Critically,
  `bind`/`bind_row` values are constrained by design (per the existing
  doc comment at lines 225-226: "bind values may only reference `$query`
  aliases or literals, not record fields") to NEVER be a field-path
  reference — there is no `FilterValue::FieldRef`-equivalent leaf that
  needs a real interner to resolve a field name to an id. The scratch
  interner in THIS path is harmless BY CONSTRUCTION, not by luck: it is
  only ever asked to resolve `$query`/`$param`/literal leaves, none of
  which perform an interner lookup keyed on a real field name.
  (Contrast `when`'s scratch interner, which IS asked to resolve field-name
  leaves via `compile_filter`, and therefore IS broken.)
- Concretely: `ForEachOp.bind_row`'s value assignment (binding the current
  loop element to a `$param` name for the body to consume) is a **direct
  assignment of a resolved value to a parameter name**, structurally
  identical to how `SubBatchOp.bind`'s non-`Param` branch resolves each
  bind entry today (`other => { resolve_filter_query(other, &dummy_record,
  &bind_ctx)... }`) — it is not a Filter-based comparison and does not
  route through `compile_filter`/`FilterNode::matches` at any point.

**Conclusion:** `ForEach`/`bind_row` does NOT inherit bug #651. The two
mechanisms happen to share the incidental detail of "construct a scratch
`Interner::new()`", but #651 is specifically about that scratch interner
being handed to `compile_filter`/`FilterNode::matches` (a `Filter`-tree
evaluator whose field-path leaves need a REAL interner to resolve field
names to ids) — a code path `bind`/`bind_row` never touches. Phase B
(#653) can implement `bind_row` resolution as a direct structural copy of
the existing `bind` resolution loop (lines 234-266) without waiting on
#651's fix and without inheriting its defect. This should still be
recorded as an explicit non-goal check in Phase D's test suite (a
regression test asserting `bind_row` resolves correctly even before #651 is
fixed), so the independence claim in this ADR is pinned by a test, not only
by this document's code-reading.

One caveat worth flagging (not a #651 inheritance, a separate, narrower
observation): if a future `ForEach`/`when` COMBINATION is added (per the
brief's own "если `ForEach` где-либо комбинируется с `when`" concern — e.g.
"skip iteration `i` if some field of the current bound row matches X"),
THAT hypothetical per-iteration `when` filter, if it references a FIELD of
the bound row (not a `$param`/`$query` literal), WOULD go through
`compile_filter`/`resolve_skip` and WOULD inherit #651 — because at that
point it is a genuine `Filter`-tree evaluation, not a `bind_row` value
assignment. This is consistent with (not contradicting) the conclusion
above: `bind_row` itself (the assignment mechanism) is clean; a
hypothetical field-based per-iteration `when` gate layered on top of it
would not be, for the same reason `when` is broken everywhere else today.
Phase B should not add per-iteration `when` support in this epic; if a
later epic does, it must block on #651 being fixed first, exactly as
Epic03's own ADR recommends for nested `$cond`/`$expr` in `when` (bug
#642).

---

## Summary of decisions

| # | Decision | Confidence |
|---|---|---|
| 1 | `BatchOp::ForEach(ForEachOp { over: FilterValue, bind_row: String, batch: BatchRequest })`; body planned once, executed K times; `over` is `FilterValue` (value-producing), not `Filter`; "repeat N" is a literal-array degenerate case, no separate wire field; `while` stays out of scope (WASM escape hatch) | High — confirmed, structural analogy to `SubBatchOp` verified directly in code |
| 2 | Loop alias `value` = `QueryValue::List` of per-iteration maps (reusing the existing single-sub-batch value shape, K times); `@loop[i].alias[j].field` indexed addressing deferred to a follow-up; `@loop[].alias` column-projection recommended as the first addressing extension when needed | Medium-high — the `List` representation is high-confidence; the addressing-extension scope/timing is a genuine open call, flagged as such |
| 3 | `BatchLimits::max_iterations`, default 1000; static `over` folds `iterations × body.len()` into the existing `max_queries` check at plan time; dynamic (`$query`-column) `over` is gated by a runtime `actual_len > max_iterations` check BEFORE iteration 0, erroring rather than truncating | High — confirmed against `BatchLimits::default()`/`planner.rs`'s existing budget check; runtime-gate-for-dynamic-case is the only architecturally possible option given the planner has no I/O |
| 4 | Tx batch: any iteration error aborts the whole plan (matches existing `?`-propagation in `execute_plan_tx_impl`). Non-tx (autocommit) batch: **stop-at-first**, matching today's ALREADY-stop-at-first behavior for independent top-level ops in `execute_plan_impl` — collect-errors would be a new, inconsistent philosophy, not a neutral choice | High — confirmed by reading `execute_plan_impl`'s `?`-based loop; no existing collect-errors precedent anywhere in the executor |
| 5 | `ForEachOp.batch` authorized as a template at plan time, pessimistic model identical to `SubBatchOp`'s existing `is_write = any nested op is_write` and Epic03's `when` precedent; iteration count (including possible zero iterations) never affects the classification | High — confirmed by reading `BatchOp::is_write`'s actual `Batch(sub)` arm and Epic03 ADR's Decision 3 |
| #651 relevance | **`bind_row` (the `ForEach` analogue of `SubBatchOp.bind`) is NOT affected by bug #651.** The bug is specific to `resolve_skip`'s `compile_filter`/`FilterNode::matches` path for `when: Filter`; `bind`/`bind_row` resolves via `resolve_filter_query` on `FilterValue` leaves and never calls `compile_filter`. A hypothetical future per-iteration field-based `when` layered on top of `ForEach` WOULD inherit #651 — flagged as a scope boundary, not a contradiction | High — confirmed by direct code read of both `resolve_skip` (lines 102-141) and the `bind`-resolution loop (lines 227-266) in `query_runner.rs` |

One open question requiring orchestrator judgment: Decision 2's addressing
scope (defer both `[i]`-indexed and `[]`-column extensions, vs. bundling
the cheaper `[]`-column form into Phase B immediately). My recommendation
is to defer both and ship `List`-only in Phase B, but this is flagged as a
genuine scope call rather than a fact established by the code, since no
code currently implements either extension to confirm or refute against.
