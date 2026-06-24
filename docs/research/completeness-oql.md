בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# OQL Completeness Assessment

**Scope:** the S.H.A.M.I.R. data query language (OQL) — the typed-object /
MessagePack DTO language, **not** a textual SQL. This report inventories what
OQL has today and what is missing relative to mature DBMS query languages
(SQL, MongoDB's MQL, Cypher, etc.), citing real file paths and type names.
"Engine supports but no language surface" is distinguished from "truly
absent". Items marked **(unverified)** were not directly confirmed against
source.

**Sources of truth read:**
`crates/shamir-query-types/src/` (filter/, read/, batch/, write/, subscribe/,
admin/, call/, auth/) · `crates/shamir-engine/src/query/` (filter/, read/,
batch/, common/) · `crates/shamir-engine/src/table/read_planner.rs` ·
`crates/shamir-engine/src/validator/schema/` · `crates/shamir-funclib/src/`
· `docs/roadmap/ROADMAP.md`, `docs/roadmap/PLAN.md` (§3 "Resolved forks").

**Guiding principle (from `docs/roadmap/PLAN.md` §3 and
`docs/roadmap/ROADMAP.md`):** OQL is **object-native by design, forever — no
text/SQL frontend will be built**. "OQL may *grow* (more operators, `$fn`,
richer filters) — that is evolving the same object language, never a textual
frontend." Therefore any gap that is really "no SQL parser" is **intentionally
out of scope by principle**, not a defect. Gaps below are judged on OQL's own
terms (does the object language express it?).

---

## 1. What OQL HAS — feature inventory

### 1.1 Read pipeline (`ReadQuery`)
Type: `ReadQuery` in `crates/shamir-query-types/src/read/read_query.rs`.
Fields: `from: TableRef`, `select: Select`, `where: Option<Filter>`,
`group_by: Option<GroupBy>`, `order_by: Option<OrderBy>`,
`pagination: Pagination`, `count_total: bool`, `temporal: Temporal`,
`with_version: bool`.

Execution pipeline (`crates/shamir-engine/src/query/read/exec.rs` + README):
```
index/full scan → WHERE → (GROUP BY → agg → HAVING)? → SELECT → DISTINCT → ORDER BY → PAGINATION
```

### 1.2 Filtering — `Filter` enum (22 logical/comparison variants + 3 index ops)
`crates/shamir-query-types/src/filter/filter_enum.rs`:
- **Comparison:** `Eq`, `Ne`, `Gt`, `Gte`, `Lt`, `Lte` (+ shortcut `FieldEq`).
- **Pattern:** `Like`, `ILike`, `Regex`.
- **Null/existence:** `IsNull`, `IsNotNull`, `Exists`, `NotExists`.
- **Set/containment:** `In`, `NotIn`, `Contains`, `ContainsAny`, `ContainsAll`.
- **Range:** `Between`.
- **Logical combinators:** `And { filters }`, `Or { filters }`, `Not { filter }` —
  arbitrary nesting up to `MAX_FILTER_DEPTH = 64` (`check_filter_depth`).
- **Index-accelerated ops:** `Fts` (full-text, AND/OR token modes),
  `VectorSimilarity` (top-k NN), `Computed` (functional-index comparison:
  lower/upper/trim/length/substring/mod × eq/lt/gt/lte/gte).

### 1.3 Filter values — `FilterValue` (rich expression model)
`crates/shamir-query-types/src/filter/filter_value.rs` — `untagged` enum:
literals (`Null/Bool/Int/Float/String/Binary/Array`), `FieldRef` (`$ref`,
same-record cross-field), `QueryRef` (`$query`, cross-batch result reference),
`FnCall` (`$fn`), `Expr` (`$expr`), `Cond` (`$cond` ternary), `Param`
(`$param`, bound from sub-batch `bind` map).

- **`FilterExpr` / `FilterExprOp`** (`filter_expr.rs`): arithmetic
  (`Add/Sub/Mul/Div/Mod/Neg`), string (`Concat/Lower/Upper/Trim/Length`),
  logic, and comparison ops returning bool.
- **`Cond`** (`cond.rs`): ternary `if/then/else` with nested conditions.
- **`FnCall`** (`fn_call.rs`): system functions, simple + complex
  (name+args) forms — examples in code: `NOW`, `UUID`, `COALESCE`,
  `SUBSTRING`.
- **`FieldPath = Vec<String>`** — nested-document paths
  (`["address","city"]`), single-string ergonomic form accepted on the wire.

### 1.4 Projection / SELECT — `Select`, `SelectItem`
`crates/shamir-query-types/src/read/select.rs`:
- `SelectItem::All` (`*`), `Field { path, alias }`, `Aggregate { func, field,
  alias, distinct }`, `CountAll`, `AggregateFn { name, field, alias, distinct }`
  (library-dispatched: median/mode/stddev/variance/percentile/count_distinct/
  string_agg/array_agg …), `Function { name, args, alias }` (per-row scalar
  call via funclib), `Expression { expr, alias }`.
- `Select.distinct: bool` — DISTINCT on the projected row.

### 1.5 Aggregation
- **Closed fast-path set** `AggFunc` (`agg.rs`): `Count, Sum, Avg, Min, Max`.
- **Library aggregates** `shamir-funclib/src/agg.rs::register`: `count`,
  `count_distinct`, `sum`, `avg`, `min`, `max`, `median`, `stddev`,
  `variance`, `percentile` (parameterised `p`), `first`, `last`,
  `string_agg` (parameterised sep), `array_agg`, `bool_and`, `bool_or`,
  `mode`, `range` — 18 aggregate factories. Distinct-aggregate modifier on
  both `Aggregate` and `AggregateFn`.
- **`GROUP BY` + `HAVING`** (`group_by.rs`): `GroupBy { fields, having:
  Option<Filter> }`. HAVING reuses the `Filter` AST (can reference aggregate
  output aliases — see `pre_intern_select_keys` in `exec.rs`).
- Engine impl: `apply_group_by` / `apply_aggregate_all` in
  `crates/shamir-engine/src/query/read/aggregate.rs` (lens-fed, zero-copy
  accumulators).

### 1.6 Scalar function library — `shamir-funclib`
`crates/shamir-funclib/src/lib.rs::register_builtins` wires 12 folders;
`ScalarRegistry` (`registry.rs`) maps folder-qualified names (`math/abs`,
`strings/lower`, …) to `FnEntry` (arity-checked, purity/determinism/trusted-pure
metadata — the functional-index safety gate).
- **~130 unique scalar functions** across: `math` (15: abs/ceil/floor/round/
  trunc/sign/neg/pow/sqrt/exp/ln/log/mod/clamp/min/max/between), `strings`
  (26: lower/upper/trim/length/byte_length/substring/concat/replace/split/
  starts_with/ends_with/contains/index_of/repeat/reverse/pad_left/pad_right
  + 8-name regex family), `arrays` (11), `cast` (8), `datetime` (23),
  `value_nav` (5), `validate` (12), `encode` (12), `object` (8), `text` (7),
  `crypto` (6), `canonical` (1).
- Functions are pure `fn(&[QueryValue]) -> ScalarResult`, callable in filters
  (`$fn`), projection (`SelectItem::Function`), and as schema-rule predicates
  (the `scalar` constraint — see §1.10).
- Regex engine: Rust `regex` crate (ReDoS-safe), pattern-cached
  (`strings.rs`).

### 1.7 Sorting & pagination
- **`OrderBy` / `OrderByItem`** (`order_by.rs`): per-item `field`, `direction`
  (Asc/Desc), `nulls: Option<NullsOrder>` (First/Last) — **explicit NULL
  ordering is supported**.
- **`Pagination`** (`limit.rs`): `LimitOffset { limit, offset }`, `Page {
  page, page_size }`, `None`. `PaginationInfo` carries `total_count`,
  `total_pages`, `current_page`, `has_next/has_prev`.
- **ORDER BY + LIMIT K fast path** (`try_plan_order_limit_fast_path` in
  `read_planner.rs`): sorted-index top-K (asc `lookup_first_k`, desc
  `lookup_last_k`) — materialises only `skip+take` entries.

### 1.8 Range scans & index planning
`crates/shamir-engine/src/table/read_planner.rs` + `query/filter/index_range.rs`:
- **Equality / In index scan** (`try_plan_index_scan`): single-field and
  composite indexes; `In` → multi-lookup union; residual filter built from
  unconsumed `And` conjuncts.
- **Sorted-index range scan** (`try_plan_sorted_index_scan` +
  `try_plan_and_range_index_scan`): `Between/Gte/Lte` direct; `Gt/Lt` via
  inclusive window + `Ne` residual.
- **SSI predicate ranges** (`predicate_to_index_range` in
  `query/filter/index_range.rs`): byte-level `PredicateDep::IndexRange` for
  serializable-isolation phantom protection.
- Index kinds (per `docs/roadmap/PLAN.md` §1 + index README): hash (regular +
  unique), sorted, **functional**, **HNSW vector**, **FTS**, **covering**
  (`include:[…]`, Opt O).

### 1.9 MVCC temporal reads — `Temporal`
`crates/shamir-query-types/src/read/temporal.rs`:
- `Latest` (default), `AsOf { at: At }` (`At::Version(u64)` or
  `At::Timestamp(u64)` epoch-millis), `History { from, to, limit, order }`
  (open-bounded version range, Asc/Desc).
- Imperative twins: `PurgeHistoryOp`, `SetRetentionOp` (`Retention`:
  `max_age_secs`/`max_count`/`min_count`), `ChangesSinceOp` (cursor-based
  journal read — the pull precursor to live subscriptions) — all in
  `crates/shamir-query-types/src/admin/types/retention.rs`.
- `with_version: bool` on `ReadQuery` — include each record's MVCC version
  (for CAS / optimistic cursors).

### 1.10 Schema / constraints (DDL-A + declarative)
`crates/shamir-engine/src/validator/schema/`:
- **`FieldRule`** (`field_rule.rs`) + **`Constraints`** (`constraints.rs`):
  `required`, `nullable`, `min/max` (Int/F64), `len/min_len/max_len`,
  `unsigned`, `one_of` (enum/const), `array_of` (typed list elements),
  `format` (email/url/uuid/date — `FormatKind`), `scalar` (scalar-bridge
  predicate), `compare` (cross-field, e.g. `start <= end`).
- **`TypeTag`**: Int/F64/String/Bool/Bin/Dec/List/Map/Set/Null/Any.
- **Foreign key** (`foreign_key.rs::ForeignKeyRef`): forward-only, existence
  checked at write time (Phase C2). NULL bypasses (SQL semantics).
- **Unique constraint** (Phase C3, `Constraints::unique`): enforced against
  committed rows + staged writes in the same tx.
- CHECK-constraint analogue = the `scalar` predicate rule (any registered
  scalar returning Bool) + `compare` cross-field rule.

### 1.11 Writes — `InsertOp`, `UpdateOp`, `SetOp`, `DeleteOp`
`crates/shamir-query-types/src/write/types.rs`:
- Insert (with `records_idmsgpack` pass-through fast path), Update (with
  `UpdateSelect { return_mode: All|Changed|Unchanged, fields }` — RETURNING
  analogue), Set (upsert by key), Delete (filter required for safety).
- All carry `TableRef` (optionally repo-qualified).

### 1.12 Batch + cross-op references — the relational substitute
`crates/shamir-query-types/src/batch/`:
- **`BatchRequest`** (`batch_request.rs`): `queries: TMap<String, QueryEntry>`
  (alias → op), `transactional`, `isolation` (`snapshot`|`serializable`),
  `durability` (`buffered`|`synced`|`async_index`), `return_all`/`return_only`,
  `limits`, `interner_epochs`, `result_encoding`.
- **`$query` cross-references** (`reference.rs::QueryReference` / `QueryPath`):
  `@alias`, `@alias[n]`, `@alias[]`, `@alias.field`,
  `@alias[0].address.city`, `@alias.count`. A read's filter can reference
  another query's result column — a **semi-join / IN-subquery equivalent**
  expressed as a batch dependency edge (planned in `batch/planner.rs`,
  executed in `query/batch/query_runner.rs`).
- **`SubBatchOp`** (`sub_batch_op.rs`): nested batch with own tx scope + `bind:
  TMap<String, FilterValue>` (the `$param` source) — recursion with depth
  guard.
- **`CallOp`** (`call/mod.rs`): stored-procedure invocation
  (`{ call, params, repo }`); `params` are `FilterValue`s, may be `$query`
  refs; result returns in `QueryResult::value`.
- **`TransactionInfo`** (`transaction_info.rs`): `tx_id`, `status`
  (committed/aborted), `reason`, `snapshot_version`, `commit_version`,
  `materialized`.

### 1.13 Live subscriptions — `SubscribeOp`
`crates/shamir-query-types/src/subscribe/`:
- `SubscribeOp { subscribe: Vec<SubscriptionSource>, deliver: DeliverMode,
  initial: bool, from_version: Option<u64> }`.
- `SubscriptionSource { table, filter: Option<Filter>, events: EventMask }`
  — per-source filtered push (Put/Delete/All).
- `DeliverMode`: `Records`, `Keys`, `Batch(SubBatchOp)` (reactive sub-batch
  with `$event.*` injection), `Call(CallOp)`.
- `UnsubscribeOp`. Cursor resumption via `from_version` +
  `ChangesSinceOp` (pull precursor).

### 1.14 Admin / DDL surface
`BatchOp` dispatch (`batch_op.rs`) carries ~40 op variants: Create/Drop for
Db/Repo/Table/Index, BufferConfig (set/get/alter), List, Migration
(start/commit/rollback/status), Auth (User/Role create/drop + grant/revoke),
Access-control (chmod/chown/chgrp/group ops/access_tree), Function DDL
(create/drop/rename/folder), Validator DDL (create/drop/rename/bind/unbind/
list), declarative schema (set_table_schema/add_schema_rule/remove_schema_rule/
get_table_schema), interner (dump/touch), temporal admin (purge_history/
set_retention/changes_since), Call, SubBatch, Subscribe/Unsubscribe.

### 1.15 Result shape & stats
`QueryResult` (`query_result.rs`): `records: Vec<QueryRecord>`,
`stats: Option<QueryStats>` (`index_used`, `records_scanned`,
`records_returned`, `execution_time_us`), `pagination: Option<PaginationInfo>`,
`value: Option<QueryValue>` (non-tabular stored-proc result).

---

## 2. What's MISSING or weak

For each: **status** (absent / partial / intentional / engine-has-no-surface),
**evidence**, **mature-DBMS baseline**.

### 2.1 JOIN (inner / outer / cross / semi / anti) — **truly absent** at the single-query level
- **Evidence:** grep for `join|JOIN|Join` across `crates/shamir-engine/src`
  returns only `path.join()`, `join_all`, `JoinHandle`, `JoinSet`,
  "disjoint", etc. — **no SQL JOIN operator**. `ReadQuery.from` is a single
  `TableRef`; there is no second source, no join predicate, no join type.
  No `SelectItem` for qualified/aliased foreign columns.
- **What exists instead:** (a) **`$query` cross-references in a batch** — a
  filter value can be `@alias[0].id` or `@alias[].id`, producing an
  IN-subquery / semi-join effect across two queries in one batch
  (`reference.rs`, `filter_value.rs::QueryRef`); (b) **FK existence checks**
  at write time (`foreign_key.rs`) — a write-time semi-join, not a read-time
  join; (c) **stored procedures / reactive sub-batches** (`CallOp`,
  `DeliverMode::Batch`) — a function can issue multiple reads and assemble a
  joined shape in code.
- **Mature baseline:** SQL `INNER/LEFT/RIGHT/FULL/CROSS JOIN`, Cypher
  pattern matching, Mongo `$lookup`.
- **Verdict:** multi-table relational joins are absent from the declarative
  read surface. The batch-`$query` mechanism covers the common
  "fetch detail by id from a parent result" pattern (semi-join), but a
  single declarative N-table join with arbitrary predicates and column
  aliasing is not expressible. Cross-table fan-out is delegated to
  application code or stored procedures.

### 2.2 Subqueries (scalar / correlated / EXISTS / derived-table) — **partial**
- **Evidence:** `$query` refs (`FilterValue::QueryRef`) give an
  **uncorrelated** subquery result: alias another query, navigate its result
  (`@users[].id`), and use it as an `In`/`Eq` value. There is **no correlated
  subquery** (the inner query cannot re-reference the outer row's fields),
  **no `EXISTS` subquery** (the `Exists` filter variant is field-existence
  within a record, not a subquery existence check — see `filter_enum.rs`),
  and **no derived table** (`FROM (SELECT …) AS t` — `from` is always a
  concrete `TableRef`).
- **Mature baseline:** SQL scalar/correlated subqueries, `EXISTS`/`NOT
  EXISTS`, CTEs as derived tables, Mongo `$lookup` pipeline stages.
- **Verdict:** uncorrelated-batch-subquery is well-supported; correlated
  subquery and EXISTS-as-subquery are absent.

### 2.3 Set operations (UNION / INTERSECT / EXCEPT) — **truly absent**
- **Evidence:** grep across `shamir-query-types` for
  `union|intersect|except` returns only `Caps intersect` (retention prose)
  and an unrelated comment — no set-operation op type. `BatchOp` has no
  union variant.
- **Mature baseline:** SQL `UNION [ALL]/INTERSECT/EXCEPT`.
- **Verdict:** set combination of two result sets is not expressible in one
  request. A stored procedure could merge in code; otherwise absent.

### 2.4 CTEs (WITH … AS, recursive CTEs) — **truly absent**
- **Evidence:** no `With`/`Cte`/`Recursive` type in `shamir-query-types`.
  The closest analogue is `SubBatchOp` (a nested batch with bound `$param`s)
  and `CallOp` (named procedures) — neither is a declarative in-request
  named subquery that can be referenced multiple times in the same logical
  plan.
- **Mature baseline:** SQL non-recursive and recursive CTEs (tree/graph
  traversal).
- **Verdict:** absent. Recursive graph traversal is delegated to stored
  procedures.

### 2.5 Window functions (ROW_NUMBER / RANK / LAG / LEAD / aggregates OVER PARTITION) — **truly absent**
- **Evidence:** grep for
  `window|partition_by|over\(|row_number|rank\(|lag\(|lead\(|PARTITION` across
  all crates returns only MVCC/time "window" prose, OS "Windows", and slice
  `.windows(n)` — **no SQL window-function construct**. No `Window`/`Over`
  type in `SelectItem`, no `partition_by` field anywhere.
- **Mature baseline:** SQL window functions (analytics: running totals,
  per-group rankings, time-series shifts).
- **Verdict:** absent. Top-K per group can be partially emulated with
  GROUP BY + ORDER BY + LIMIT in a sub-batch, but general framed window
  analytics are not available.

### 2.6 DISTINCT ON (distinct over a subset of columns / first-per-group) — **absent**
- **Evidence:** `Select.distinct` is a single bool applied to the whole
  projected row (`select.rs`). No `distinct_on` field; grep for
  `distinct_on|DistinctOn` returns nothing.
- **Mature baseline:** PostgreSQL `DISTINCT ON (col)`, Mongo dedup stages.
- **Verdict:** whole-row DISTINCT only; per-column / first-per-group
  distinct is absent (workaround: GROUP BY + first/last aggregate).

### 2.7 Keyset / cursor pagination — ✅ **DONE** (см. `DONE.md`)
- **Реализовано:** `Pagination::After { key, limit }` (wire-тег `"After"`) +
  engine sorted-index seek (строго-после ASC / строго-до DESC, exclusive) +
  Rust `Query::after` / TS `.after` билдеры; e2e зелёный. Изначальный gap
  (только offset → deep-page O(offset)) закрыт.
- **Verdict:** offset pagination only; deep-page O(offset) cost remains.
  Engine has the index capability but no DTO surface — a clear
  "engine-ready, language-absent" gap.

### 2.8 LIMIT on the write/return side (RETURNING beyond UPDATE) — **partial**
- **Evidence:** `UpdateOp` has `UpdateSelect { return_mode, fields }`
  (RETURNING analogue). `InsertOp` has `InsertedRecord` (in `write/mod.rs`)
  but no `RETURNING` clause in the DTO; `DeleteOp` returns affected info via
  `WriteResult` but no selectable returning columns. (Unverified: whether
  `WriteResult` always carries the full affected rows or just counts.)
- **Mature baseline:** SQL `RETURNING *` / `RETURNING cols` on
  INSERT/UPDATE/DELETE.
- **Verdict:** UPDATE has it; INSERT/DELETE returning is asymmetric / weaker.

### 2.9 Computed / generated / persisted columns — **partial (expression only)**
- **Evidence:** `SelectExpr` (`select_expr.rs`) supports arithmetic
  expressions in projection (`Add/Sub/Mul/Div`, `Field`, `Literal`). The
  doc-comment says "future: computed fields". There is **no DDL to declare
  a stored generated column** (no `generated`/`computed` in `Constraints` or
  `FieldRule`). Functional indexes (`Computed` filter, `functional` index
  kind) compute on read, not as a persisted column.
- **Mature baseline:** SQL generated columns (STORED/VIRTUAL), Mongo
  `$addFields` persistence.
- **Verdict:** ad-hoc projection expressions exist; declared generated
  columns do not.

### 2.10 Type coercion / CAST explicitness — **partial**
- **Evidence:** the `cast` funclib folder (8 functions) provides explicit
  casts. Implicit coercion is scattered: `arg_i64/arg_f64/arg_dec` in
  `registry.rs` coerce numeric variants; `compare::compare`
  (`shamir-funclib/src/compare.rs`) defines a cross-type total order. There
  is **no single documented coercion matrix** in the language; behaviour is
  defined per-function. (Unverified: whether filter `Eq` between Int and
  String coerces or returns false.)
- **Mature baseline:** SQL CAST/CONVERT with a documented precedence ladder.
- **Verdict:** ad-hoc per-operator coercion; no language-level cast grammar
  beyond the `cast/` scalar folder.

### 2.11 Geo / spatial types & predicates — **truly absent**
- **Evidence:** no `geo`/`spatial`/`point`/`polygon`/`st_` types in
  `TypeTag`, no spatial index kind, no spatial filter operator. The vector
  index (`VectorSimilarity`, HNSW) is embedding-similarity, **not** geo.
- **Mature baseline:** PostGIS, MongoDB geo, SQLite R*Tree.
- **Verdict:** absent (and not on any roadmap doc read).

### 2.12 Graph / recursive traversal — **truly absent** (intentional at the language level)
- **Evidence:** no graph traversal primitive in `Filter`/`SelectItem`. The
  roadmap (`docs/roadmap/PLAN.md` §0.2, §2) names the "I" frontier as
  network changefeed → replication → P2P, **not** a graph query layer.
- **Verdict:** absent; recursive needs are met by stored procedures
  (`CallOp`) or client-side iteration.

### 2.13 EXPLAIN / query plan introspection — **partial**
- **Evidence:** `QueryStats` returns `index_used`, `records_scanned`,
  `records_returned`, `execution_time_us` — post-hoc stats. There is **no
  EXPLAIN op** to preview the plan without executing (no `BatchOp::Explain`).
- **Mature baseline:** SQL `EXPLAIN [ANALYZE]`.
- **Verdict:** runtime stats present; dry-run plan preview absent.

### 2.14 Transaction control verbs (BEGIN / COMMIT / ROLLBACK as language) — **partial (batch-scoped)**
- **Evidence:** transactions are expressed via `BatchRequest.transactional +
  isolation + durability` (auto-commit boundaries around a batch) and via
  interactive multi-call (`query/batch/interactive_tx.rs`). There is no
  standalone `BEGIN`/`COMMIT`/`SAVEPOINT` DTO op in `BatchOp`. (Unverified:
  whether the interactive-tx path exposes savepoints.)
- **Mature baseline:** SQL `BEGIN/COMMIT/ROLLBACK/SAVEPOINT`.
- **Verdict:** transactional semantics are rich (SI/SSI/wound-wait) but the
  language surface is batch-flag + interactive-tx handle, not SQL-style
  verbs.

### 2.15 MERGE / conditional upsert with predicates — **partial**
- **Evidence:** `SetOp` is upsert-by-key (`key` + `value`, merge on update).
  `UpdateOp` is filter-based update. There is no single op that does
  "if exists and <predicate> then update else insert" atomically
  (SQL `MERGE` / Mongo upsert-with-filter).
- **Verdict:** key-upsert exists; conditional-merge is absent (workaround:
  tx + read + branch).

### 2.16 Full-text search ranking / highlighting — **partial**
- **Evidence:** `Filter::Fts` (AND/OR token modes) is index-accelerated
  (`read_planner.rs::try_plan_index2`, `fts.rs` tokeniser). There is **no
  rank/score in the projection** (no `SelectItem` for FTS relevance) and
  **no highlight/fragment return**. Docs (`docs/roadmap/FULL_TEXT_SEARCH.md`)
  are referenced as a hardening track, not yet ranked retrieval.
- **Mature baseline:** PostgreSQL ts_rank/ts_headline, ES _score.
- **Verdict:** boolean FTS present; ranked/retrieval-grade FTS is a roadmap
  hardening item.

### 2.17 Aggregation GROUP BY ROLLUP/CUBE/GROUPING SETS — **absent**
- **Evidence:** `GroupBy` has `fields: Vec<FieldPath>` only — no
  `rollup`/`cube`/`grouping_sets` modifier.
- **Mature baseline:** SQL multi-dimensional aggregates.
- **Verdict:** absent; emulate with multiple GROUP BY queries in a batch.

### 2.18 PIVOT / UNPIVOT / cross-tab — **absent**
- **Evidence:** no pivot op type. (Standard gap vs SQL Server / warehouse
  engines.)
- **Verdict:** absent; delegate to application / stored procedure.

### 2.19 Regular expression in projection / capture extraction — **present via funclib**
- **Evidence:** the `strings` folder exposes `is_reg_match reg_query
  reg_query_all reg_captures reg_replace reg_split reg_count reg_find_index`
  (8 regex functions), usable in `$fn` and `SelectItem::Function`. ReDoS-safe
  (Rust `regex`). This is **stronger** than many SQL dialects.
- **Verdict:** present (not a gap).

### 2.20 NULL handling in filters (three-valued logic) — **partial**
- **Evidence:** explicit `IsNull`/`IsNotNull`; `NullsOrder` (First/Last) in
  ORDER BY. The `compare` module defines a total order (not SQL's
  three-valued logic). Aggregators skip Nulls by SQL convention
  (`agg.rs::is_null`). (Unverified: whether `Eq field Null` matches Null
  values or always returns false — SQL says unknown.)
- **Mature baseline:** SQL three-valued logic with documented `NULL = NULL`
  semantics.
- **Verdict:** NULLs are first-class in predicates and ordering; the exact
  comparison-vs-Null semantics are total-order, not SQL-3VL (intentional
  design choice — mark as a documented divergence, not a defect).

---

## 3. PRIORITIZED gap list

Tier rationale: **High** = blocks common analytical/relational workloads that
clients likely need and cannot trivially work around. **Medium** = real gap
with a usable workaround (batch composition, stored proc). **Low** = niche,
intentionally out of scope, or adequately covered by a workaround.

### HIGH (5)

| # | Gap | One-line rationale | Impact |
|---|-----|--------------------|--------|
| H1 | **Multi-table JOIN** (inner/outer/cross) | The single most-requested relational primitive; `$query` semi-joins cover parent→detail but not arbitrary predicate joins or outer joins. | Blocks porting any relational schema/report to OQL; forces client-side N+1 or stored-proc fan-out. |
| H2 | **Window functions** (ROW_NUMBER/RANK/LAG/LEAD OVER PARTITION) | Per-group analytics (running totals, rankings, time-series shifts) have no declarative path; GROUP BY cannot express framed computation. | Blocks common BI / leaderboard / time-series queries. |
| ~~H3~~ | ~~**Keyset / cursor pagination DTO**~~ | ✅ **DONE** — `Pagination::After` + engine seek + Rust/TS билдеры (см. `DONE.md`). | — |
| H4 | **Set operations** (UNION/INTERSECT/EXCEPT) | Combining two result sets (e.g. "ids in both", "ids in A not in B") is inexpressible in one request. | Common dedup/diff workloads need client-side merge or a stored proc. |
| H5 | **Correlated subquery / EXISTS-as-subquery** | `$query` is uncorrelated only; `Exists` is field-existence, not subquery existence; no derived table. | "Users who have at least one order > $100" needs a batch + IN-ref, not a single declarative query. |

### MEDIUM (7)

| # | Gap | One-line rationale | Impact |
|---|-----|--------------------|--------|
| M1 | **CTEs** (non-recursive + recursive) | No named in-request subquery reuse; recursive graph/tree traversal absent. | Workaround = stored proc / client iteration; loses single-plan optimisation. |
| M2 | **Generated / computed columns** (DDL) | Projection expressions exist; no persisted/virtual generated column declaration. | Denormalised/precomputed fields must be maintained by app logic or functional index (read-time). |
| M3 | **FTS ranking / score / highlight** | Boolean token FTS is solid; no relevance ranking or snippet return in projection. | Search-result quality / UX below ES/Postgres ts_rank; roadmap hardening item. |
| M4 | **GROUP BY ROLLUP/CUBE/GROUPING SETS** | Only flat GROUP BY; no multi-dimensional subtotals. | Emulate with N queries in a batch; loses single-scan efficiency. |
| M5 | **EXPLAIN / dry-run plan** | `QueryStats` is post-hoc; no preview-the-plan-without-executing op. | Hard to tune queries before running on production-sized data. |
| M6 | **MERGE / conditional upsert** | `SetOp` upserts by key only; no "if <predicate> then update else insert". | Atomic conditional write needs tx+read+branch. |
| M7 | **RETURNING symmetry** (INSERT/DELETE) | `UpdateSelect` exists for UPDATE; INSERT/DELETE returning is weaker/asymmetric. | Read-after-write round-trips for INSERT/DELETE. |

### LOW (6)

| # | Gap | One-line rationale | Impact |
|---|-----|--------------------|--------|
| L1 | **DISTINCT ON (subset)** | Whole-row DISTINCT exists; per-column first-per-group absent. | Workaround: GROUP BY + first/last aggregate. |
| L2 | **Geo / spatial types & indexes** | Absent; not on roadmap. Embedding-vector ≠ geo. | Use case not served; add via a future `geo` index kind if needed. |
| L3 | **Graph / recursive traversal language** | Absent; recursive needs met by stored procs. | Niche; the "I" frontier is replication, not graph. |
| L4 | **PIVOT / UNPIVOT** | Absent; standard warehouse gap. | Emulate in application / proc. |
| L5 | **SQL-style transaction verbs** | Transactions are batch-flag + interactive-tx handle (rich semantics); no `BEGIN/SAVEPOINT` DTO op. | Intentional object-native design; interactive tx covers most needs. |
| L6 | **Documented type-coercion matrix / 3VL** | Coercion is per-operator; `compare` is total-order, not SQL three-valued logic. | Documented divergence, not a defect; mark explicitly in language spec. |

---

## 4. Summary observations

- **OQL is a deliberately object-native language** (`docs/roadmap/PLAN.md` §3):
  the DTO **is** the wire **is** the AST. Any "gap" that is really "no textual
  SQL" is **out of scope by principle**, not a missing feature.
- **Within its object-native remit, OQL is broad and mature on the
  single-table axis**: rich filter algebra (22 + 3 index ops), full expression
  model (`$expr`/`$cond`/`$fn`/`$ref`/`$query`/`$param`), GROUP BY + HAVING,
  18 aggregates, ~130 scalars, NULL-ordering, top-K + range + covering + FTS +
  vector indexes, MVCC temporal (AsOf/History/ChangesSince), subscriptions,
  nested batches, stored procedures, FK/unique/CHECK-equivalent schema.
- **The structural gap is multi-table / multi-set declarative composition**:
  JOIN, window functions, set operations, CTEs, correlated subqueries. The
  project substitutes **batch composition (`$query`)** + **stored procedures
  (`CallOp`)** + **reactive sub-batches (`DeliverMode::Batch`)** for these —
  a coherent design choice, but one that pushes relational composition to the
  application/procedure layer rather than the declarative language.
- **One clear "engine-ready, language-absent" item:** keyset pagination
  (H3) — the sorted-index seek machinery already exists; only the DTO surface
  is missing. Cheapest high-impact win.
- **Roadmap alignment:** none of H1–H5 appear on the read roadmap
  (`docs/roadmap/`); the live frontier is Movement C (replication / "I"),
  not query-language breadth. This suggests these gaps are **accepted**, not
  overlooked — consistent with the "don't over-build; pull by real need"
  discipline (`PLAN.md` §4).
