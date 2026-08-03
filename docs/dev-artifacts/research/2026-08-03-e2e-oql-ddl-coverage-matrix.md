# TS E2E Coverage Inventory + Gap Matrix — OQL & DDL

**Phase 1 deliverable (task #964).** A real inventory of the entire OQL and DDL
surface as expressed in the wire types, mapped against the TypeScript e2e test
suites, to drive per-gap follow-up `crush` sessions. This document does NOT
modify any test or production code — it is a durable read-only artifact.

**Date:** 2026-08-03
**Author:** crush (phase-1 investigation session)

---

## 0. Methodology, source of truth, and a key scope correction

### Source of truth (wire capabilities)
The capability set is enumerated from the **actual Rust wire DTOs**, not from
memory or the client builders:

- **OQL:** `crates/shamir-query-types/src/filter/filter_enum.rs` (`Filter`),
  `filter_value.rs` (`FilterValue`), `read/{read_query,select,select_expr,agg,
  group_by,order_by,limit,temporal,query_result}.rs`, `batch/{batch_op,
  for_each_op,sub_batch_op,batch_request}.rs`, `wire/{db_message,cursor_id}.rs`.
- **DDL:** `admin/types/{index_ops,db_ops,repo_ops,table_ops,buffer_config,
  migration_ops,validator_ops,schema_ops,repl_ops,list_ops,function_ops,
  interner_ops,retention}.rs`, `admin/access.rs`, `auth/types.rs`, plus the
  `BatchOp` dispatch enum in `batch/batch_op.rs` (the authoritative list of
  every admin/DDL wire op) and the top-level `DbRequest` enum in
  `wire/db_message.rs`.

### Test surface — there are TWO live-server e2e suites (not one)
The brief framed "the 18-file suite" as the e2e surface. That is only half the
picture. There is a **second, larger live-server e2e suite** that must be part
of any honest gap analysis:

1. **JS e2e suite** — `tests/e2e/tests/*.js` (18 files, query-builder-based,
   run via `node tests/e2e/e2e.test.js`). Baseline: 130 tests.
2. **TS e2e suite** — `crates/shamir-client-ts/src/__tests__/e2e-*.test.ts`
   (~26 files), run via `vitest run`. Each spawns a real `shamir-server`
   release binary through the shared `e2e-harness.ts` (confirmed: the harness
   resolves `shamir-server` / `shamir-server.exe`, opens an ephemeral port,
   and gates every suite behind `describe.skipIf(!SERVER_AVAILABLE)`). This
   suite closes a *large* fraction of the gaps the JS-only view would report
   — e.g. `e2e-data.test.ts` alone adds live-server coverage for
   `like`/`ilike`/`regex`, `contains*`, `exists`/`notExists`, `Page` mode,
   `distinct`, scalar `select.func`, `aggregateFn`, and `History` temporal
   reads.

**Coverage rule used in this matrix:** a capability is **covered (YES)** if a
real live-server e2e test exercises it in *either* suite. A pure
builder-shape unit test in `core/builders/__tests__/*.test.ts` does **NOT**
count as e2e coverage — those tests assert the serialized wire shape only and
never touch a server. (They are nevertheless strong "the client can build it"
evidence and are noted where relevant.)

### Conventions in the tables
- **E2E?** = `YES` (live server, either suite) · `NO` (no live-server test
  anywhere) · `PARTIAL` (live-server test exists but does not exercise all
  variants/options of the capability — the un-exercised options are the gap).
- **Test file:name** uses `js:` for the JS suite and `ts:` for the TS suite.
- `Gap` column flags the genuine uncovered items that become follow-up tasks.

---

## 1. OQL coverage matrix

### 1.1 Filters — `Filter` enum (`filter/filter_enum.rs`)

| # | Capability | Source location | E2E? | Test file:name | Gap? | Notes |
|---|------------|-----------------|------|----------------|------|-------|
| 1 | `Eq` | `filter_enum.rs:16` | YES | js:`05-filters` "eq"; ts:`e2e-data`/`e2e.test.ts` | — | comparison leaf |
| 2 | `Ne` | `filter_enum.rs:21` | YES | js:`05-filters` "ne"; ts:`e2e.test.ts` | — | |
| 3 | `Gt` | `filter_enum.rs:26` | YES | js:`05-filters`; ts:`e2e-data` nested age>35 | — | |
| 4 | `Gte` | `filter_enum.rs:31` | YES | js:`05-filters`; ts:`e2e-data` idx≥0 | — | |
| 5 | `Lt` | `filter_enum.rs:36` | YES | js:`05-filters`; ts:`e2e-data` qty<10 | — | |
| 6 | `Lte` | `filter_enum.rs:41` | YES | js:`05-filters` | — | |
| 7 | `Like` | `filter_enum.rs:48` | YES | ts:`e2e-data` "like — prefix match" | — | |
| 8 | `ILike` | `filter_enum.rs:53` | YES | ts:`e2e-data` "ilike — case-insensitive" | — | |
| 9 | `Regex` | `filter_enum.rs:58` | YES | ts:`e2e-data` "regex — matches pattern" | — | |
| 10 | `IsNull` | `filter_enum.rs:65` | YES | ts:`e2e-data`; ts:`e2e-when` | — | |
| 11 | `IsNotNull` | `filter_enum.rs:69` | YES | ts:`e2e-data`; ts:`e2e-when`; (delete-all in `e2e-data`) | — | |
| 12 | `In` | `filter_enum.rs:75` | YES | js:`05-filters`; ts:`e2e-data`/`e2e.test.ts` | — | |
| 13 | `NotIn` | `filter_enum.rs:80` | YES | js:`05-filters`; ts:`e2e.test.ts` | — | |
| 14 | `Contains` | `filter_enum.rs:85` | YES | ts:`e2e-data` "contains" | — | array field |
| 15 | `ContainsAny` | `filter_enum.rs:90` | YES | ts:`e2e-data` "containsAny" | — | |
| 16 | `ContainsAll` | `filter_enum.rs:95` | YES | ts:`e2e-data` "containsAll" | — | |
| 17 | `Between` | `filter_enum.rs:102` | YES | js:`05-filters`; ts:`e2e-data` | — | inclusive |
| 18 | `Exists` | `filter_enum.rs:110` | YES | ts:`e2e-data` "exists" | — | field-present |
| 19 | `NotExists` | `filter_enum.rs:114` | YES | ts:`e2e-data` "notExists" | — | field-absent |
| 20 | `And` | `filter_enum.rs:120` | YES | js:`05-filters`; ts:many | — | incl. nested |
| 21 | `Or` | `filter_enum.rs:123` | YES | js:`05-filters`; ts:many | — | incl. nested |
| 22 | `Not` | `filter_enum.rs:126` | YES | js:`05-filters`; ts:`e2e-data` | — | |
| 23 | `FieldEq` | `filter_enum.rs:131` (`serde rename "field"`) | NO | — | **low** | Wire shortcut for `field == value`; functionally identical to `Eq`, so it is implicitly exercised by every `eq` test that the server deserializes — but no test asserts the `field` op tag specifically. Low-priority gap. |
| 24 | `ValueCompare` (+ `ValueCompareOp` Eq/Ne/Gt/Gte/Lt/Lte) | `filter_enum.rs:146`, `:207` | YES | ts:`e2e-cond` (valueGte/valueLt inside `$cond`) | — | value-vs-value; used in `when` guards |
| 25 | `Fts` (mode `and`/`or`) | `filter_enum.rs:158` | YES | js:`14-index2-types` fts AND/OR; ts:`e2e-fts` | — | |
| 26 | `VectorSimilarity` (`k`, `ef_search`, `oversample`) | `filter_enum.rs:176` | YES | js:`18-vectors`; ts:`e2e-vector` | — | efSearch clamp + filtered ANN + oversample all covered |
| 27 | `Computed` (expr_op lower/upper/trim/length/substring/mod/coalesce/concat; cmp; expr_args) | `filter_enum.rs:190` | PARTIAL | js:`14-index2-types` (functional `lower`/`upper` lookup only) | **medium** | Only `lower`/`upper` exercised; `trim`/`length`/`substring`/`mod`/`coalesce`/`concat` and `expr_args` never hit a server. |

**Filter subtotal: 27 capabilities — 24 fully covered, 1 partial, 1 low-priority gap (FieldEq), 1 zero-coverage variant-bundle inside Computed.**

### 1.2 `FilterValue` dynamic-value variants (`filter/filter_value.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `QueryRef` (`$query`) | `filter_value.rs:25` | YES | js:`04-batch-deps`; ts:`e2e-batch-sequencing`/`e2e-cond` | — | `[N].field` + `[].field` IN-expansion |
| `FieldRef` (`$ref`) | `filter_value.rs:20` | YES | ts:`e2e-data` (`select.func` args use `filter.ref`) | — | |
| `Cond` (`$cond`) | `filter_value.rs:43` | YES | ts:`e2e-cond` (3-level nesting, branch→prior result) | — | |
| `Param` (`$param`) | `filter_value.rs:49` | YES | ts:`e2e-for-each` (`bind_row`) | — | |
| `FnCall` (`$fn`) | `filter_value.rs:33` | PARTIAL | ts:`e2e-call` (`call` op with params) | **low** | `$fn` in **filter/select** value context (e.g. `filter.fn`) not clearly exercised end-to-end against a live server; only the `Call` op path is. |
| `Expr` (`$expr`) | `filter_value.rs:38` | NO | — | **low** | No live-server test of a `$expr` FilterValue. |
| Literal `Binary` | `filter_value.rs:17` | NO | — | **low** | No e2e inserts/filters a binary value. |

### 1.3 Projections — `SelectItem` enum (`read/select.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `All` (`*`) | `select.rs:53` | YES | everywhere (default select) | — | |
| `Field` (+ alias) | `select.rs:56` | YES | js:`06-projections-aggregations`; ts:`e2e-data` | — | incl. nested path |
| `Aggregate` (Count/Sum/Avg/Min/Max + field/alias/**distinct**) | `select.rs:63` | PARTIAL | js:`06`; ts:`e2e-data` "count/sum/avg/min/max" | **low** | The five funcs are covered; the per-aggregate `distinct: true` flag (e.g. `SUM(DISTINCT x)`) is NOT exercised. `COUNT(DISTINCT …)` is instead covered via `aggregateFn` (1.4). |
| `CountAll` (+ alias) | `select.rs:73` | YES | js:`06`; ts:`e2e-data` | — | |
| `AggregateFn` (funclib: median/mode/stddev/variance/percentile/count_distinct/string_agg/array_agg…) + `args` | `select.rs:85` | PARTIAL | ts:`e2e-data` "aggregateFn: count_distinct" | **medium** | Only `count_distinct` exercised; parameterised aggregates (`percentile` p, `string_agg` sep) and the rest of the funclib set never hit a server. |
| `Function` (scalar projection, folder-qualified e.g. `strings/upper`) + `args` | `select.rs:105` | YES | ts:`e2e-data` "select.func: strings/upper", "strings/length" | — | |
| `Expression` (`SelectExpr` Add/Sub/Mul/Div/Field/Literal) | `select.rs:124`, `select_expr.rs` | **N/A** | — | — | **DELIBERATELY out of scope:** the executor REJECTS `SelectItem::Expression` at runtime (F-26 / #819, `select.rs:114` doc). The wire shape is accepted but no evaluator exists. Not an e2e gap until the feature ships. |

### 1.4 Aggregation / grouping (`read/agg.rs`, `read/group_by.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `AggFunc::{Count,Sum,Avg,Min,Max}` | `agg.rs:10` | YES | js:`06`; ts:`e2e-data` | — | |
| `AggregateField::All` (`*`) | `agg.rs:24` | YES | `countAll` | — | |
| `GroupBy` (fields) | `group_by.rs:9` | YES | js:`06`; ts:`e2e-data` "group_by tag" | — | |
| `GroupBy.having` | `group_by.rs:13` | NO | — | **high** | No live-server test of a HAVING clause anywhere. |
| `Select.distinct` (whole-row dedup) | `select.rs:14` | YES | ts:`e2e-data` "distinct" | — | |

### 1.5 Sorting (`read/order_by.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `OrderDirection::Asc` | `order_by.rs:76` | YES | js:`07`; ts:many | — | |
| `OrderDirection::Desc` | `order_by.rs:79` | YES | js:`07`; ts:`e2e.test.ts` | — | |
| composite (multi-item) | `order_by.rs:10` | YES | js:`07` "multiple fields"; ts:`e2e.test.ts` | — | |
| `NullsOrder::First` / `Last` | `order_by.rs:85` | NO | — | **medium** | No e2e asserts NULL placement. |

### 1.6 Pagination (`read/limit.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `LimitOffset` (limit + offset) | `limit.rs:18` | YES | js:`07`; ts:`e2e.test.ts` | — | |
| `Page` (page/page_size) | `limit.rs:27` | YES | ts:`e2e-data` "page(1,3)…(4,3)" | — | |
| `After` (keyset/seek) | `limit.rs:44` | YES | ts:`e2e-keyset` (sorted-index seek) | — | |
| `After.after_id` tie-breaker (#537) | `limit.rs:77` | NO | — | **medium** | The builder emits it (`query.test.ts`), but no live-server test verifies tie-breaking past a shared ORDER-BY value. |
| `count_total` (→ `PaginationInfo.total_count`) | `read_query.rs:33` | YES | js:`07`; ts:`e2e.test.ts` | — | |
| `PaginationInfo` (`total_pages`, `has_next`, `current_page`) | `limit.rs:264` | PARTIAL | js:`07` (total_count) | **low** | Only `total_count` is asserted; `total_pages`/`has_next`/`current_page` not checked. |

### 1.7 Temporal & metadata (`read/temporal.rs`, `read/read_query.rs`, `read/query_result.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `Temporal::Latest` (default) | `temporal.rs:27` | YES | implicit everywhere | — | |
| `Temporal::AsOf { Version }` | `temporal.rs:29`+`At::Version:15` | YES | ts:`e2e-data` "asOfVersion reads historical state" | — | |
| `Temporal::AsOf { Timestamp }` | `temporal.rs:29`+`At::Timestamp:17` | YES | ts:`e2e-data` "asOfTimestamp" | — | |
| `Temporal::History` (from/to/limit/order) | `temporal.rs:33` | PARTIAL | ts:`e2e-data` "history range asc/desc + limit" | **low** | `order` asc/desc + `limit` covered; `from`/`to` window bounds not specifically asserted. |
| `with_version` (→ `QueryResult.versions`) | `read_query.rs:41`+`query_result.rs:162` | YES | ts:`e2e-data`; ts:`e2e-version-cas` | — | |
| `explain` (dry-run → `QueryResult.explain`/`ExplainPlan`/`PlanType`) | `read_query.rs:45`+`query_result.rs:41..51` | NO | — | **high** | No live-server test runs a dry-run plan preview. `PlanType` has 9 variants (`KeysetSeek`,`OrderLimitFast`,`Index2`,`IndexScan`,`SortedIndexScan`,`AndRangeIndexScan`,`CounterShortcut`,`MinMaxIndex`,`FullScan`) — none asserted via explain, though several are indirectly observable via `stats.index_used`. |
| `stats.index_used` reporting | `query_result.rs:57` | YES | js:`14`/`18` ("index2_ranked"); ts:`e2e-vector` | — | |

### 1.8 Batch DAG / multi-op (`batch/*`, `wire/db_message.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| multi-query batch (independent/parallel) | `batch_request.rs` | YES | js:`03-batch-multi`; ts:`e2e.test.ts` | — | |
| `queryRef` cross-reference + stage DAG | `filter_value.rs:25`,`batch/reference.rs` | YES | js:`04-batch-deps`; ts:`e2e-batch-sequencing`/`e2e-cond` | — | `[N].field`, `[].field` IN-expansion, 3-step chains |
| explicit `after` deps + edge_provenance | `batch/query_entry.rs` | YES | ts:`e2e-batch-sequencing` | — | incl. circular-dep detection |
| `SubBatch` nested (recursive tx scope + `bind`) | `batch/sub_batch_op.rs` | YES | ts:`e2e-for-each` body; ts:`e2e-subscriptions` deliver-handle | — | |
| `ForEach` loop (`over`/`bind_row`, incl. `$query`/`$fn` over) | `batch/for_each_op.rs` | YES | ts:`e2e-for-each` (literal array + `$query` col-ref + zero-iters + mid-loop tx rollback) | — | |
| `when` guards / `switchCase` (`QueryResult.skipped`) | query_entry + `query_result.rs:144` | YES | ts:`e2e-when` (multi-branch switchCase, `skipped` detection) | — | |
| batch `limits` (6 fields incl. `max_iterations`) | `batch/batch_limits.rs` | YES | ts:`e2e-batch-limits` | — | |
| `transactional` + `isolation` (snapshot/serializable) | `batch/batch_request.rs` | YES | js:`15-transactions`; ts:`e2e-data`/`e2e-when` | — | |
| `durability` (`synced`/`async_index`) | batch_request | NO | — | **low** | Builder-only (`batch.test.ts`); no live-server assertion of durability flag effect. |

### 1.9 Cursors (`wire/db_message.rs`, `wire/cursor_id.rs`)

| Capability | Source | E2E? | Test file:name | Gap? | Notes |
|------------|--------|------|----------------|------|-------|
| `CreateCursor` (first page + `CursorId`) | `db_message.rs:222` | YES | ts:`e2e-cursors` | — | |
| `FetchNext` (with/without `page_size`) | `db_message.rs:239` | YES | ts:`e2e-cursors` | — | |
| `CancelCursor` | `db_message.rs:257` | YES | ts:`e2e-cursors`/`e2e-cursor-lifecycle` | — | incl. early-break `IteratorClose` |
| idle-timeout eviction / per-session cap | server config | YES | ts:`e2e-cursor-lifecycle` (`cursor_expired`, `cursor_limit_exceeded`) | — | |
| cursor + AsOf rejection | engine | YES | ts:`e2e-cursors` (`cursor_temporal_not_supported`) | — | |

### OQL subtotal
~**60 distinct capabilities** enumerated (filters + filter-values + projections +
aggregation/group + sort + pagination + temporal/metadata + batch-DAG + cursors).
**~46 fully covered**, ~**9 partial**, **~7 outright gaps** (HAVING, EXPLAIN,
`after_id` tie-breaker, NULLS ordering, `$expr` FilterValue, binary literals,
batch `durability`) plus the deliberately-out-of-scope `SelectItem::Expression`.

---

## 2. DDL coverage matrix

Capabilities sourced from `BatchOp` (`batch/batch_op.rs:41`) — the exhaustive
admin/DDL dispatch enum — and the typed op structs in `admin/types/*`.

### 2.1 Database / repo / table lifecycle

| Capability | Wire key / source | E2E? | Test file:name | Gap? | Notes |
|------------|-------------------|------|----------------|------|-------|
| create db | `create_db` / `db_ops.rs` | YES | js:`08`/`10`; ts:`e2e-ddl`/many | — | |
| drop db (+ `cascade`) | `drop_db` / `db_ops.rs` | YES | js:`10`/`12`; ts:`e2e-ddl` | — | incl. `still_referenced` w/o cascade |
| **rename db** | `rename_db` / `db_ops.rs` (`BatchOp:63`) | NO | — | **medium** | `RenameDbOp` is a real wire op; no live-server test anywhere. |
| create repo | `create_repo` / `repo_ops.rs` | YES | js:`08`; ts:many | — | |
| drop repo | `drop_repo` / `repo_ops.rs` | YES | ts:`e2e-ddl` | — | |
| rename repo | `rename_repo` / `repo_ops.rs` | YES | ts:`e2e-rename-repo` | — | data preserved under new name |
| create table | `create_table` / `table_ops.rs` | YES | everywhere | — | |
| drop table | `drop_table` / `table_ops.rs` | YES | js:`12`; ts:many | — | HMAC-gated |
| rename table | `rename_table` / `table_ops.rs` | YES | ts:`e2e-rename-table` | — | |
| **describe_table** | `describe_table` / `table_ops.rs` (`BatchOp:121`) | NO | — | **medium** | Single-call full introspection op; never hit a server. |

### 2.2 Index lifecycle — `CreateIndexOp` (`admin/types/index_ops.rs:23`)

| Capability | `CreateIndexOp` field / source | E2E? | Test file:name | Gap? | Notes |
|------------|--------------------------------|------|----------------|------|-------|
| create regular (hash) index | default | YES | js:`08`; ts:`e2e-ddl`/`e2e-rename-index` | — | |
| create `unique` index (+ constraint) | `unique` `:28` | YES | ts:`e2e-for-each` | — | |
| create `sorted` index (range/order/min) | `sorted` `:32` | YES | ts:`e2e-keyset` | — | backs keyset seek |
| drop index (+ HMAC, `unique` flavor) | `DropIndexOp` `:115` | YES | js:`12` "drop_index unique=true tag flavour"; ts:`e2e-ddl` | — | |
| rename index (all families) | `RenameIndexOp` `:99` | YES | ts:`e2e-rename-index` | — | posting-list migration verified |
| `index_type=fts`, tokenizer `whitespace` | `index_type:38`,`fts_tokenizer:42` | YES | js:`14-index2-types`; ts:`e2e-fts` | — | |
| `fts_tokenizer=unicode` | `:42` | NO | — | **medium** | Only `whitespace` exercised; `unicode` tokenizer never hit. |
| `fts_language` (stemming hint) | `:46` | NO | — | **low** | Referenced in a comment in `14-index2-types` only; never executed. |
| `index_type=functional`, `functional_op` lower/upper | `functional_op:51` | YES | js:`14-index2-types` "LOWER/UPPER lookup" | — | |
| functional ops `trim/length/substring/mod/coalesce/concat` + `functional_args` | `functional_op:51`,`functional_args:55` | NO | — | **medium** | Only `lower`/`upper` exercised; remaining 6 ops + parameterised args never tested. |
| `index_type=vector`, `vector_dim`, metric `cosine` | `vector_dim:59`,`vector_metric:63` | YES | js:`14`/`18`; ts:`e2e-vector` | — | |
| `vector_metric` `l2` / `dot` | `:63` | YES | js:`18`; ts:`e2e-vector` | — | |
| `vector_quantization=sq8` | `:70` | YES | js:`18` "sq8"; ts:`e2e-vector` | — | incl. >256-vector fit threshold |
| `include` (covering fields, sorted only) | `:76` | NO | — | **medium** | Covering-index field projection never tested. |
| `if_not_exists` (create) | `:81` | YES | ts:`e2e-ddl` | — | |
| `if_exists` (drop) | `DropIndexOp:128` | PARTIAL | builder-only (`ddl.test.ts`) | **low** | No live-server assertion of the silent no-op return. |
| composite (multi-`fields`) index | `:26` (`Vec<Vec<String>>`) | PARTIAL | mostly single-field | **low** | Multi-column composite indexes not systematically exercised. |

### 2.3 Buffer config (`admin/types/buffer_config.rs`)

| Capability | Wire key | E2E? | Test file:name | Gap? |
|------------|----------|------|----------------|------|
| set_buffer_config | `set_buffer_config` | YES | js:`11-buffer-config`; ts:`e2e-ddl` | — |
| get_buffer_config | `get_buffer_config` | YES | js:`11`; ts:`e2e-ddl` | — |
| alter_buffer_config (partial patch; `ttl_ms:null` clears) | `alter_buffer_config` | YES | js:`11` (3-state patching); ts:`e2e-ddl` | — |

### 2.4 Migrations (`admin/types/migration_ops.rs`)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| start_migration | `start_migration` | YES | js:`13-migration`; ts:`e2e-ddl` | — | HMAC-gated, `dst_engine` validation |
| commit_migration | `commit_migration` | YES | js:`13`; ts:`e2e-ddl` | — | |
| rollback_migration | `rollback_migration` | YES | js:`13`; ts:`e2e-ddl` | — | |
| migration_status | `migration_status` | YES | js:`13`; ts:`e2e-ddl` | — | |

### 2.5 Validators (`admin/types/validator_ops.rs`)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| create_validator | `create_validator` | YES | ts:`e2e-ddl` | — | |
| bind_validator | `bind_validator` | YES | ts:`e2e-ddl` | — | |
| unbind_validator | `unbind_validator` | YES | ts:`e2e-ddl` | — | |
| list_validators | `list_validators` | YES | ts:`e2e-ddl` | — | |
| drop_validator | `drop_validator` | YES | ts:`e2e-ddl` | — | |
| **rename_validator** | `rename_validator` | NO | — | **low** | Only validator op with no live-server test. |

### 2.6 Declarative schema (`admin/types/schema_ops.rs`, field builder)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| set_table_schema (+ constraints) | `set_table_schema` | YES | ts:`e2e-schema-validators` | — | type/min/max/required/nullable/one_of/format/compare/unique/fk |
| **get_table_schema** | `get_table_schema` | NO | — | **low** | Set is covered; the read-back op is not asserted. |
| **add_schema_rule** | `add_schema_rule` | NO | — | **low** | `e2e-schema-validators` only uses `setTableSchema` with embedded rules; the incremental `add_schema_rule` op never executed. |
| **remove_schema_rule** | `remove_schema_rule` | NO | — | **low** | Same — incremental removal op never executed. |
| field constraints (type tags, foreign_key onDelete/onUpdate, default expr, auto_now) | schema field builder | YES | ts:`e2e-schema-validators` (+ builder `ddl.test.ts`) | — | |
| expected_version on setTableSchema | schema `expected_version` | PARTIAL | ts:`e2e-version-cas` (CAS on writes); builder `ddl.test.ts` | **low** | Schema-version CAS not directly asserted on `setTableSchema`. |

### 2.7 Access control / ACL (`admin/access.rs`, `admin/mod.rs`)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| chmod | `chmod` | YES | js:`16`/`17`; ts:`e2e-permissions` | — | |
| chown | `chown` | YES | ts:`e2e-permissions`/`e2e-principal` | — | resolves principal64 |
| chgrp | `chgrp` | YES | ts:`e2e-permissions` | — | |
| access_tree | `access_tree` | YES | ts:`e2e-permissions`/`e2e-principal` | — | |
| resolvePrincipal (client helper, NOT a wire op) | `client.ts:1023` | YES | ts:`e2e-principal` | — | Client-side derivation from accessTree; included for completeness. |

### 2.8 Groups (`admin/types/*.rs` group ops)

| Capability | Wire key | E2E? | Test file:name | Gap? |
|------------|----------|------|----------------|------|
| create_group | `create_group` | YES | ts:`e2e-permissions`/`e2e-principal` | — |
| drop_group | `drop_group` | YES | ts:`e2e-permissions` | — |
| rename_group | `rename_group` | YES | ts:`e2e-permissions` | — |
| add_group_member | `add_group_member` | YES | ts:`e2e-principal` | — |
| remove_group_member | `remove_group_member` | YES | ts:`e2e-permissions` | — |

### 2.9 Users / RBAC (`auth/types.rs`, top-level `DbRequest`)

| Capability | Wire surface | E2E? | Test file:name | Gap? | Notes |
|------------|--------------|------|----------------|------|-------|
| create_user (db-level) | `BatchOp::CreateUser` | YES | ts:`e2e-permissions` | — | |
| create_scram_user (login) | `DbRequest::CreateScramUser` `db_message.rs:57` | YES | js:`16-replication`; ts:`e2e-permissions` | — | HMAC-gated |
| drop_user | `BatchOp::DropUser` | YES | ts:`e2e-permissions` | — | |
| grant_role | `grant_role` | YES | ts:`e2e-permissions` | — | |
| revoke_role | `revoke_role` | YES | ts:`e2e-permissions` | — | |
| set_superuser | `DbRequest::SetSuperuser:188` | YES | ts:`e2e-permissions` | — | HMAC-gated |
| set_replicator | `DbRequest::SetReplicator:203` | YES | js:`16-replication` | — | task #931 |
| list_users | `ListOp::Users` (`list_ops.rs:29`) | YES | ts:`e2e-ddl` | — | |
| **change_password** (challenge/verify) | `DbRequest::ChangePasswordChallenge:147`+`ChangePasswordVerify:159` | NO | — | **medium** | Full 2-step SCRAM password-change wire path untested end-to-end. |

### 2.10 Functions (`admin/types/function_ops.rs`)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| create_function (source/wasm, replace, visibility, security) | `create_function` | YES | ts:`e2e-ddl`/`e2e-call` | — | |
| drop_function | `drop_function` | YES | ts:`e2e-ddl` | — | |
| rename_function | `rename_function` | YES | ts:`e2e-ddl` | — | |
| create_function_folder | `create_function_folder` | YES | ts:`e2e-ddl` | — | |
| rename_function_folder | `rename_function_folder` | PARTIAL | builder-only | **low** | create/list covered; rename folder not executed live. |
| list_functions (+ folder filter) | `ListOp::Functions` | YES | ts:`e2e-ddl` | — | |
| list_function_folders | `ListOp::FunctionFolders` | YES | ts:`e2e-ddl` | — | |
| call (stored proc, params incl. `$ref`/`$query`) | `BatchOp::Call` / `call.rs` | YES | ts:`e2e-call` | — | |

### 2.11 Interner / temporal-admin / retention (`admin/types/{interner_ops,retention}.rs`)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| interner_touch (client `touchFields`) | `interner_touch` | YES | ts:`e2e-interner`/`e2e-data` | — | cache warm + id round-trip |
| **interner_dump** | `interner_dump` | NO | — | **low** | The dump wire op itself never executed (only client cache tested). |
| set_retention | `set_retention` | YES | ts:`e2e-ddl` | — | HMAC-gated |
| purge_history (olderThan/currentOnly/olderThanAge) | `purge_history` | YES | ts:`e2e-ddl` (olderThanAge) | — | |
| changes_since (journal read) | `changes_since` | YES | ts:`e2e-ddl` | — | |

### 2.12 Replication (`admin/types/repl_ops.rs`, `wire/repl.rs`)

| Capability | Surface | E2E? | Test file:name | Gap? | Notes |
|------------|---------|------|----------------|------|-------|
| ReplHello (privileged wire) | `DbRequest::Repl`→`ReplRequest` | YES | js:`16-replication` | — | replicator role, leader_epoch |
| ReplPull (events stream) | `ReplRequest` | YES | js:`16-replication` | — | |
| create_publication + repl_scope | `create_publication` | YES | js:`17-replication-convergence` | — | |
| create_replication_profile + repl_stream | `create_replication_profile` | YES | js:`17` | — | |
| create_subscription | `create_subscription` | YES | js:`17` | — | leader→follower convergence |
| **drop_publication** | `drop_publication` | NO | — | **medium** | |
| **drop_subscription** | `drop_subscription` | NO | — | **medium** | |
| **drop_replication_profile** | `drop_replication_profile` | NO | — | **low** | |
| **alter_subscription** (pause/resume/set_profile) | `alter_subscription` | NO | — | **medium** | |
| **list_publications** | `list_publications` | NO | — | **medium** | |
| **list_subscriptions** | `list_subscriptions` | NO | — | **medium** | |
| **replication_status** | `replication_status` | NO | — | **medium** | |
| two-server convergence | integration | YES | js:`17-replication-convergence` | — | |

> Replication is the single largest DDL gap cluster: 7 lifecycle/introspection
> ops (drop×3, alter, list×2, status) have builder + msgpack-parity coverage
> (`repl_parity.test.ts`) but **zero** live-server execution.

### 2.13 Subscribe / unsubscribe (`subscribe/*`)

| Capability | Wire key | E2E? | Test file:name | Gap? | Notes |
|------------|----------|------|----------------|------|-------|
| subscribe (where filter, event mask, deliver modes, initial, from_version) | `subscribe` | YES | ts:`e2e-subscriptions` | — | incl. handle deliver + initial snapshot |
| unsubscribe | `unsubscribe` | YES | ts:`e2e-subscriptions` | — | |

### 2.14 HMAC gate & interactive transactions

| Capability | Surface | E2E? | Test file:name | Gap? | Notes |
|------------|---------|------|----------------|------|-------|
| hmac_required / hmac_mismatch on destructive ops | engine | YES | js:`12-hmac-gate`; js:`13-migration` | — | target-bound tag verified |
| interactive tx TxBegin/TxExecute/TxCommit/TxRollback | `DbRequest:89..132` | NO/uncertain | — | **medium** | `js:15-transactions` covers the **batch** `transactional` flag, NOT the multi-call interactive `TxBegin`/`TxExecute` wire path. No dedicated live-server test of the interactive-tx lifecycle was found. **Needs confirmation.** |

### DDL subtotal
~**78 distinct capabilities** enumerated across 14 DDL families. **~58 fully
covered**, ~**6 partial**, **~15 outright gaps** — concentrated in replication
lifecycle (7), index2 option breadth (functional ops/tokenizer/covering), and
the `rename_db` / `describe_table` / `change_password` / interactive-tx
singletons.

---

## 3. Prioritized gap clusters → follow-up tasks

Grouped into natural, separately-scoped `crush`-session clusters. Priority is
driven by (a) how much of the wire surface is unexercised and (b) correctness
risk of the unexercised path. Each cluster is sized to one focused session.

### Cluster A — Replication lifecycle & introspection ops e2e  *(HIGH)*
**Why:** largest single gap. 7 wire ops (drop_publication, drop_subscription,
drop_replication_profile, alter_subscription pause/resume/set_profile,
list_publications, list_subscriptions, replication_status) have builder +
byte-parity coverage but **no live-server execution**. The convergence harness
in `js:17` already proves a 2-server setup, so the scaffolding exists.
**Scope:** extend `tests/e2e/tests/16-replication.test.js` (or a new TS file)
to exercise create→list→alter→drop for publication/subscription/profile and
assert `replication_status` output. Source: `repl_ops.rs`, `repl_parity.test.ts`
for exact wire shapes.

### Cluster B — Aggregate `HAVING` + `EXPLAIN` dry-run + `PlanType` assertions  *(HIGH)*
**Why:** HAVING and EXPLAIN are both completely unexercised live (zero hits
across both suites) yet are first-class `ReadQuery`/`GroupBy` fields with 9
`PlanType` variants. EXPLAIN is the only way to assert the planner picked the
right index strategy — currently inferred only indirectly via `stats.index_used`.
**Scope:** new tests: group_by + having predicate (re-use `e2e-data.test.ts`
group-by data), and `explain:true` queries asserting `QueryResult.explain`
(`plan_type`, `index_used`, `estimated_rows`) across sorted/fts/vector/index
paths. Source: `group_by.rs:13`, `read_query.rs:45`, `query_result.rs:41`.

### Cluster C — Functional-index & FTS option breadth  *(MEDIUM)*
**Why:** only `lower`/`upper` functional ops and `whitespace` fts tokenizer
are exercised. The wire supports functional ops `trim/length/substring/mod/
coalesce/concat` (+ `functional_args`), fts tokenizer `unicode`, and
`fts_language` — all untested server-side.
**Scope:** extend `tests/e2e/tests/14-index2-types.test.js` to create
functional indexes for each remaining `functional_op` (incl. parameterised
`mod`/`substring` via `functional_args`) and exercise the matching `Computed`
filter; add an fts index with `unicode` tokenizer + a `language` hint.
Source: `index_ops.rs:42..55`, `filter_enum.rs:190`.

### Cluster D — Index2 DDL extras: covering `include`, composite, drop `if_exists`  *(MEDIUM)*
**Why:** `include` (covering-index field projection, sorted-only) is entirely
untested; composite multi-column indexes and `if_exists` drop semantics are
only builder-covered.
**Scope:** new tests asserting (1) a sorted index with `include` fields serves
a covered range query without a data fetch (observable via `stats`), (2)
multi-column composite index equality/range, (3) `drop_index {if_exists:true}`
returns `{existed:false}` on a missing index. Source: `index_ops.rs:76`,`DropIndexOp:128`.

### Cluster E — DDL singletons: `rename_db`, `describe_table`, `change_password`, interactive tx  *(MEDIUM)*
**Why:** four real wire surfaces with **zero** live-server coverage:
`RenameDbOp`, `DescribeTableOp` (full single-call introspection),
`DbRequest::ChangePasswordChallenge`/`Verify` (2-step SCRAM), and the
interactive `TxBegin`/`TxExecute`/`TxCommit`/`TxRollback` path (`js:15` only
covers the batch `transactional` flag).
**Scope:** one file per op family — rename_db round-trip (data intact under new
db name); describe_table returns schema+indexes+validators+retention+buffer;
change_password full challenge→verify→re-login; interactive tx open→exec→
commit/rollback with `tx_handle`. ⚠️ Confirm interactive-tx status first — it
may have partial coverage worth locating before writing new tests.
Source: `db_ops.rs`, `table_ops.rs DescribeTableOp`, `db_message.rs:89..211`.

### Cluster F — Keyset `after_id` tie-breaker + NULLS ordering + pagination metadata  *(MEDIUM)*
**Why:** the #537 `after_id` record-id tie-breaker (the fix for rows dropped at
page boundaries on shared ORDER-BY values) is builder-covered but never verified
live; `NullsOrder::{First,Last}` is entirely untested; `PaginationInfo`
fields (`total_pages`,`has_next`,`current_page`) are never asserted.
**Scope:** seed rows sharing an ORDER-BY value across a page boundary, page
with and without `after_id`, assert the tied rows are no longer dropped; add
NULL-containing data and assert `nulls_first`/`nulls_last`; assert the full
`PaginationInfo` shape from a `count_total` query. Source: `limit.rs:77`,`order_by.rs:85`,`limit.rs:264`.

### Cluster G — Misc low-priority OQL gaps  *(LOW)*
A grab-bag of small, individually-low-risk gaps; bundle into one session:
- `FieldEq` (`op:"field"`) dedicated assertion.
- `$expr` FilterValue + `$fn` in filter/select value context (only the `Call`
  op path is e2e'd today).
- Binary literal values in insert/filter round-trip.
- `distinct:true` flag on an `Aggregate` SelectItem (`SUM(DISTINCT x)`).
- Funclib `AggregateFn` breadth beyond `count_distinct` (`percentile` with
  `args`, `string_agg`, `median`, `stddev`).
- `History` temporal `from`/`to` window bounds (only `order`/`limit` covered).
- batch `durability` (`synced`/`async_index`) live effect.
Source: `filter_enum.rs:131`,`filter_value.rs:33..42`,`select.rs:63`,`select.rs:85`,`temporal.rs:33`.

### Cluster H — Misc low-priority DDL gaps  *(LOW)*
Bundle into one session:
- `rename_validator`, `rename_function_folder` live execution.
- `get_table_schema`, `add_schema_rule`, `remove_schema_rule` (incremental
  schema-mutation ops — currently only `setTableSchema` with embedded rules).
- `interner_dump` wire op.
- `fts_language` hint.
- `expected_version` CAS on `setTableSchema`.
Source: `validator_ops.rs`,`schema_ops.rs`,`interner_ops.rs`,`index_ops.rs:46`.

---

## 4. Summary numbers

| Surface | Capabilities enumerated | Fully covered (live) | Partial | Outright gaps |
|---------|------------------------|----------------------|---------|---------------|
| **OQL** | ~60 | ~46 | ~9 | ~7 (+ `Expression` deliberately excluded) |
| **DDL** | ~78 | ~58 | ~6 | ~15 |
| **Total** | **~138** | **~104** | **~15** | **~22** |

**Headline:** coverage is broader than the JS-only view suggested (the TS e2e
suite quietly closes ~20 capabilities), but **22 outright gaps** remain,
dominated by **replication lifecycle (7)**, **index2 option breadth**,
**HAVING/EXPLAIN**, and the **rename_db / describe_table / change_password /
interactive-tx** singletons. The 8 clusters above are the proposed follow-up
task breakdown.
