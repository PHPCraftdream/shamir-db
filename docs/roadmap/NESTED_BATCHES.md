# NESTED BATCHES — composable sub-batches as batch operations (#282)

**Status:** APPROVED design — implementing (revision 2026-06-09).

**Key motivation:** "подождать транзакцию" — a sub-batch runs as its own
transactional unit; outer ops that reference it via `$query` **wait** for
its commit and see durable results. The outer batch is an **orchestrator**;
transactional atomicity lives inside sub-batches.

**Approved decisions:**
- True nesting (recursive batch executor), NOT flatten.
- Data into sub-batch: explicit `bind` params (variant P) + `$param` values.
  Sub-batch is self-contained (no lexical scoping of parent aliases).
- Data out: sub-batch alias is a normal `QueryResult`; `$query @sub[...]`.
- TX-in-TX: forbidden on phase 1 (sub-batch inside already-transactional
  parent → error `nested_tx_not_supported`). Savepoints — future.
- Three atoms: `BatchOp::Batch(SubBatchOp)`, `FilterValue::Param`,
  `SubBatchOp.bind` map.

A batch can be included in another batch as a normal operation. Its
named results are available to other operations in the **outer** batch
via the existing `$query` dependency mechanism. This closes the
composition gap: today reads, writes, DDL, and `Call` compose; a
sub-batch composes the same way.

---

## §0 — Problem statement

A client that needs to reuse an existing multi-query workflow inside a
larger batch has two options today — both inferior:

1. **Inline every inner op.** Flatten the inner batch's queries into
   the outer `queries` map under unique aliases and rewire every
   `$query` reference by hand. This works but is error-prone,
   duplicates logic, and defeats the purpose of the builder.
2. **Two round-trips.** Execute the inner batch first, extract the
   values on the client, feed them into the outer batch. This wastes a
   network round-trip and loses atomicity for transactional batches.

**The gap:** a batch entry whose payload is itself a batch, whose
results are addressable from siblings via `$query`.

---

## §1 — Status quo (what exists today)

### 1.1 Wire shape

A `BatchRequest` is a flat map of named operations:

```
BatchRequest {                                           // types.rs:587
    id, name, transactional, isolation, durability,
    queries: TMap<String, QueryEntry>,                   // key = alias
    return_all, return_only, limits: BatchLimits,
}
```

Each `QueryEntry` holds a `#[serde(flatten)] op: BatchOp` plus
`return_result` and `after` fields (`types.rs:524–541`).

### 1.2 BatchOp dispatch

`BatchOp` is a 40+ variant enum detected by a unique JSON key
(`types.rs:179–388`). The deserialiser walks `contains_key` checks in
priority order — `from` → `insert_into` → `update` → `delete_from` →
DDL ops → `set` (last, because `UpdateOp` also has a `set` field).

Custom `Serialize` / `Deserialize` impls; no `#[serde(untagged)]`.

### 1.3 Planner / dependency DAG

`BatchPlanner::plan` (`planner.rs:84–147`):

1. Walks every `QueryEntry` and extracts `$query` refs from filters,
   set values, insert values, and `Call` params
   (`planner.rs:150–293`). The helper `extract_base_alias`
   (`planner.rs:303–308`) strips `@` and cuts at `[` / `.`.
2. Validates all referenced aliases exist in the flat `queries` map
   (`planner.rs:113–120`).
3. Detects cycles via DFS white-gray-black (`planner.rs:311–357`).
4. Calculates max dependency depth (`planner.rs:360–393`), checked
   against `BatchLimits::max_dependency_depth` (default 10)
   (`types.rs:797`).
5. Topological-sorts into parallel stages (`planner.rs:399–433`).

Output: `BatchPlan { stages: Vec<Vec<String>>, aliases, dependencies }`
(`types.rs:835–844`).

### 1.4 Executor

`execute_batch` (`executor.rs:75–163`) runs the plan:

1. Cross-repo guard for transactional batches (`executor.rs:86–93`).
2. Plan → validate tables → validate filter depth (`executor.rs:96–102`).
3. Branch: `execute_plan` (non-tx) or `execute_plan_tx` (tx-aware)
   (`executor.rs:107–124`).

Both `execute_plan` / `execute_plan_tx` iterate stages sequentially;
within each stage they iterate aliases sequentially (parallelism within
a stage is a future opt, `executor.rs:338–347`). For each alias:

1. Build `resolved_refs` — only the declared deps, not all accumulated
   results (`executor.rs:672–685`).
2. `QueryRunner::run` dispatches by variant and evaluates `$query` refs
   through `FilterContext::resolved_refs`
   (`eval_context.rs:17–24`, `eval.rs:129–133`).

### 1.5 `$query` resolution

`FilterValue::QueryRef { alias, path }` (`filter_value.rs:24–30`)
is resolved in two places:

- **Filter evaluation** (`eval.rs:129–187`) — `resolve_query_ref_value`
  walks a `QueryResult` by path (`[0].field`, `[].field`, `.field`).
  Call results use `QueryResult.value`; Read results use `.records`.
- **Call params** (`execute.rs:2156–2210`) — `filter_value_to_query_value`
  resolves `QueryRef` against `resolved_refs` for stored-procedure args.

### 1.6 Builder surface

**Rust** (`batch/mod.rs` in `shamir-query-builder`):

- `Batch` accumulates `QueryEntry`s under string aliases (`line 268`).
- Every `query()` / `insert()` / … call returns a `Handle` (`line 69`)
  whose `column()`, `row()`, `first()`, `all()` emit
  `FilterValue::QueryRef` values (`lines 83–107`).
- `try_build` (`line 823`) validates `$query` refs client-side.

**TS** (`batch.ts` in `shamir-client-ts`):

- `Batch.add(alias, op)` adds a raw op; returns `this` (no Handle).
- `filter.queryRef(alias, path?)` (`filter.ts:211–214`) builds
  `{ $query: alias, path? }`.

### 1.7 Transactions / isolation

A transactional batch opens one `TxContext` per repo, runs all ops
inside it, and commits (`executor.rs:440–559`). The cross-repo guard
(`types.rs:460–465`, `executor.rs:86–93`) rejects a transactional
batch that touches > 1 repo. `isolation` / `durability` are set on the
outer `BatchRequest` only.

### 1.8 `return_result` / `return_only`

`filter_results` (`executor.rs:935–950`) applies `return_all` /
`return_only` / per-entry `return_result` after execution. Silent
entries (via `query_silent` / `op_silent`) are the standard way to
stage intermediate results without bloating the response.

---

## §2 — Wire / types design

### 2.1 New `BatchOp` variant

```rust
/// Nested sub-batch: a full BatchRequest embedded as an operation.
///
/// Wire key: `"queries"` (a map, not a scalar — unambiguous).
Batch {
    queries: TMap<String, QueryEntry>,
    // Inherited from BatchRequest but scoped to the sub-batch:
    return_all: bool,            // default true
    return_only: Option<Vec<String>>,
}
```

The discriminating key is `"queries"` — a JSON object is recognised as a
sub-batch when it contains the key `"queries"` (a map). This is unique:
no other `BatchOp` variant serialises a `queries` field at the top
level. The planner's key-dispatch chain in `types.rs:187` gains one
check:

```rust
} else if obj.contains_key("queries") {
    serde_json::from_value(value)
        .map(BatchOp::Batch)
        .map_err(serde::de::Error::custom)
}
```

Position: before the final `set` fallback. Order among sibling keys
(`from`, `insert_into`, …) is unchanged.

**Why not reuse `BatchRequest` directly?** `BatchRequest` carries `id`,
`transactional`, `isolation`, `durability`, and `limits` — fields that
belong to the outer batch's lifecycle. A sub-batch should not open its
own transaction or carry its own durability flag; it runs within the
outer batch's execution context. Using a trimmed struct avoids
accidental misuse and keeps the serde shape clean.

### 2.2 Serde shape (wire)

```json
{
  "id": 1,
  "queries": {
    "user": { "from": "users", "where": { "op": "eq", "field": ["id"], "value": 42 } },
    "sub": {
      "queries": {
        "orders": { "from": "orders", "where": {
          "op": "eq", "field": ["user_id"],
          "value": { "$query": "@user", "path": "[0].id" }
        }},
        "items": { "from": "order_items", "where": {
          "op": "in", "field": ["order_id"],
          "values": [{ "$query": "@orders", "path": "[].id" }]
        }}
      }
    },
    "stats": { "from": "stats", "where": {
      "op": "eq", "field": ["item_count"],
      "value": { "$query": "@sub", "path": "items.count" }
    }}
  }
}
```

### 2.3 How the outer batch addresses the sub-batch's results

The sub-batch's **alias** in the outer `queries` map (e.g. `"sub"`)
acts as a namespace. The `$query` reference syntax gains one extension:

| Syntax | Meaning |
|--------|---------|
| `{ "$query": "@sub" }` | The entire sub-batch result map (all named inner aliases) |
| `{ "$query": "@sub.orders", "path": "[0].id" }` | `orders[0].id` from the sub-batch |
| `{ "$query": "@sub", "path": "items.count" }` | `items` result's `count` (pathed into the result map) |

The `QueryReference` parser (`reference.rs:118`) already handles
`@alias.path` — the `alias` is `"sub.orders"`. The planner's
`extract_base_alias` (`planner.rs:303`) currently stops at `.` which
would split `"sub.orders"` into base `"sub"` — this is **correct** for
the topological order (the outer entry depends on the sub-batch entry
`"sub"`). Resolution at eval time walks deeper: `resolved_refs["sub"]`
yields a `QueryResult` whose `value` is the sub-batch's results map,
and the `.orders[0].id` tail is resolved through the existing
`resolve_json_path` machinery (`eval.rs:215+`).

**Result shape for a sub-batch:** `QueryResult` with `value =
Some(<results map as JSON>)` — a JSON object keyed by the sub-batch's
inner aliases, each value being the inner `QueryResult` serialised.
`records` is empty; `stats` / `pagination` are `None`.

---

## §3 — Builder surface

### 3.1 Rust

```rust
// batch/mod.rs — new method on Batch
impl Batch {
    /// Add a nested sub-batch under `alias`.
    ///
    /// Returns a `SubBatchHandle` whose `result(alias)` and
    /// `column(alias, field)` produce `FilterValue::QueryRef` values
    /// that the planner sees as dependencies on the outer entry.
    pub fn batch(&mut self, alias: impl Into<String>, inner: Batch) -> SubBatchHandle {
        let op = BatchOp::Batch {
            queries: inner.queries,
            return_all: inner.return_all,
            return_only: inner.return_only,
        };
        self.add_entry(alias, op, true)
        // SubBatchHandle wraps the same alias but emits dotted paths.
    }
}

pub struct SubBatchHandle { outer_alias: String }

impl SubBatchHandle {
    /// Reference the entire sub-batch result map.
    pub fn all(&self) -> FilterValue { qref_all(&self.outer_alias) }

    /// Reference a named inner result with a path.
    pub fn result(&self, inner_alias: &str, path: impl IntoFieldPath) -> FilterValue {
        let dotted_alias = format!("{}.{}", self.outer_alias, inner_alias);
        let segments = path.into_field_path();
        let path_str = format!("[].{}", segments.join("."));
        qref(&dotted_alias, path_str)
    }

    /// Reference a single value from an inner result.
    pub fn first(&self, inner_alias: &str, field: impl IntoFieldPath) -> FilterValue {
        let dotted_alias = format!("{}.{}", self.outer_alias, inner_alias);
        let segments = field.into_field_path();
        let path_str = format!("[0].{}", segments.join("."));
        qref(&dotted_alias, path_str)
    }
}
```

### 3.2 TypeScript

```typescript
// batch.ts — new method
class Batch {
  addBatch(alias: string, inner: Batch): this {
    const innerBuilt = inner.build();
    this.queriesMap[alias] = {
      queries: innerBuilt.queries,
      return_all: innerBuilt.return_all,
      return_only: innerBuilt.return_only,
    };
    return this;
  }
}

// filter.ts — no change needed; queryRef already accepts dotted aliases:
filter.queryRef('sub.orders', '[0].id')
```

The TS builder does not return a `Handle` (it returns `this`), so
`SubBatchHandle` is not needed. The caller composes the alias string
themselves, using `queryRef`.

---

## §4 — Planner / executor changes

### 4.1 Recommended approach: **flatten into the outer DAG**

**Why flatten (vs recursive sub-plans):**

| Aspect | Flatten | True nesting (recursive sub-plans) |
|--------|---------|-----------------------------------|
| Planner change | Small: expand `BatchOp::Batch` entries before planning | Large: recursive `BatchPlan`, sub-stage injection |
| Cycle detection | Reuse existing DFS across all ops | Need cross-boundary cycle detection |
| Depth limit | Single `max_dependency_depth` across the whole tree | Ambiguous: per-level vs cumulative? |
| Executor change | None (same flat `execute_plan`) | New recursive dispatch in `QueryRunner` |
| Result addressing | Direct `$query @sub.inner` | Requires indirection through sub-result |
| `resolved_refs` | No change | Sub-plan must merge inner refs into outer |

**Flatten** is clearly better: it reuses the entire existing planner,
executor, and resolution pipeline unchanged. The sub-batch is "just" a
compile-time grouping that the planner expands.

### 4.2 Flattening algorithm

Before calling `BatchPlanner::plan`, the executor (or a pre-planning
step) expands `BatchOp::Batch` entries:

```rust
fn flatten_batch(request: &BatchRequest, limits: &BatchLimits)
    -> Result<(TMap<String, QueryEntry>, Vec<String>), BatchError>
{
    let mut flat: TMap<String, QueryEntry> = new_map();
    // For each outer entry:
    //   - If not BatchOp::Batch, insert as-is under its alias.
    //   - If BatchOp::Batch, insert each inner entry under a
    //     namespaced alias "outer_alias.inner_alias".
    //     Preserve inner's after/deps by namespacing them too.
    //     Mark the sub-batch container itself as synthetic (not in the
    //     plan); its results are synthesized from inner results.
    // Return the flat map + the list of sub-batch container aliases
    // (for result aggregation).
}
```

**Namespacing convention:** an inner entry with alias `"orders"` inside
outer entry `"sub"` becomes `"sub.orders"` in the flat map. This is
safe because the planner's `extract_base_alias` stops at `.` — a
`$query` ref to `"@sub.orders"` has base alias `"sub"`, which maps to
the synthetic container. The planner validates that `"sub"` exists as a
key. The container entry is a no-op `QueryEntry` that depends on all
its children; when the executor reaches it, it synthesises a
`QueryResult` by aggregating its children's results.

**Alias rules:**

- Outer aliases must not contain `.` (enforced at deserialization time,
  or validated in the planner).
- Inner aliases are prefixed with `"outer."`, so they cannot collide
  with outer aliases or siblings.

### 4.3 `max_dependency_depth` across nesting

The depth is calculated over the **flattened** DAG, so a chain
`A → sub.B → sub.C → D` counts as depth 3. The existing limit (default
10) applies uniformly. No per-nesting-level limit is needed.

### 4.4 `max_queries` limit

The flattened map's total entry count is checked against
`BatchLimits::max_queries`. A deeply nested batch that expands to 200
entries hits the limit exactly as a flat 200-entry batch would.

### 4.5 Cycle detection

Existing DFS (`planner.rs:311–357`) operates on the flattened
dependency map. A cross-boundary cycle (`A → sub.B → A`) is detected
naturally because all aliases are in the same flat namespace.

---

## §5 — Transactions / isolation

### 5.1 Sub-batch shares the outer tx

A `BatchOp::Batch` variant does **not** carry `transactional` /
`isolation` / `durability` fields. It executes within whatever context
the outer batch provides:

- **Non-tx outer:** sub-batch ops run non-tx (autocommit each write).
- **Tx outer:** sub-batch ops run inside the outer's `TxContext` via
  `execute_plan_tx`.

No sub-transaction, no nested `begin_tx`. This is the simplest correct
model and matches how every other `BatchOp` variant works.

### 5.2 Cross-repo guard

The guard (`distinct_repos`, `types.rs:460–465`) already walks
`BatchOp::table_ref()`. The `BatchOp::Batch` variant returns `None`
from `table_ref()` (it's a container, not a data op). The flattening
step expands it into child entries whose `table_ref()` is checked
normally. Result: the guard works without change on the flattened map.

### 5.3 `return_result` / `return_only`

- The sub-batch container is always `return_result: true` (the caller
  asked for its results).
- Inner entries inherit their own `return_result` flag.
- `return_only` on the sub-batch trims which inner results appear in
  the synthesised response.
- The outer batch's `return_only` / `return_all` controls whether the
  sub-batch container appears in the final `BatchResponse.results`.

---

## §6 — Macro impact

**None.** Neither `shamir-sdk-macros` nor `shamir-query-builder-macros`
contains batch-related code (audit confirmed — only doc-comment
references). The new `BatchOp::Batch` variant is a pure data change in
`shamir-query-types`. No proc-macro changes needed.

---

## §7 — Test / e2e / bench plan

### 7.1 Unit tests

| Test | Location | What it covers |
|------|----------|----------------|
| `nested_batch_serde_roundtrip` | `shamir-query-types/batch/tests/` | `BatchOp::Batch` serialises/deserialises via `"queries"` key |
| `nested_batch_dispatch` | same | Key-dispatch disambiguates `"queries"` from `"from"`, `"set"`, etc. |
| `flatten_basic` | `shamir-engine/query/batch/tests/` | Two-op sub-batch flattens into two namespaced entries |
| `flatten_cross_boundary_dep` | same | Inner `$query` refs to outer aliases are preserved |
| `flatten_cycle_across_boundary` | same | Cycle spanning nesting levels is detected |
| `flatten_depth_limit` | same | Deep nesting exceeds `max_dependency_depth` |
| `nested_result_addressing` | same | `$query @sub.inner[0].id` resolves correctly |

### 7.2 Integration / e2e tests

| Test | Location | What it covers |
|------|----------|----------------|
| `nested_batch_read_chain` | `shamir-db/tests/` | Outer read → sub-batch (read + read) → outer read using sub result |
| `nested_batch_write_then_read` | same | Sub-batch writes, outer reads the written data (non-tx) |
| `nested_batch_transactional` | same | Tx outer wraps sub-batch with writes; all-or-nothing on conflict |
| `nested_batch_silent_inner` | same | `return_result: false` on inner entry omits it from sub-result |
| `nested_batch_return_only` | same | Sub-batch `return_only` trims inner results |

### 7.3 Benchmarks

| Bench | Location | What it measures |
|-------|----------|------------------|
| `nested_batch_vs_flat` | `shamir-db/benches/` | Same 10-op workload: flat batch vs 2-level nested. Expected: < 1 µs overhead (flattening is in-memory) |
| `nested_batch_deep_3_levels` | same | 3-level nesting, 30 total ops. Measures flattening + planning time. |

---

## §8 — Open questions

### O1 — Maximum nesting depth

Should there be a hard limit on nesting levels (e.g. max 3), or is the
existing `max_queries` (default 50) sufficient? A malicious client
could send `{"queries":{"a":{"queries":{"b":{"queries":…}}}}}` — each
level adds 1 flattened entry, so `max_queries` bounds total work. A
separate `max_nesting_depth` in `BatchLimits` is defence-in-depth.

**Recommendation:** add `max_nesting_depth: usize` to `BatchLimits`
(default 4). Checked during flattening. Low cost, high safety.

### O2 — Error propagation from a failed sub-batch

If one inner op fails, does the entire sub-batch fail (atomic from the
outer's perspective), or does the outer see partial results?

**Options:**
- **A) Fail the sub-batch container.** The outer batch receives a
  `BatchError::QueryError` for the sub-batch alias. Other outer ops
  that depend on the sub-batch also fail. Simple and consistent with
  how single-op failures work today.
- **B) Partial success.** The sub-batch result includes errors per
  inner alias. Complex; requires a new result shape.

**Recommendation:** Option A. The executor already aborts on first
error (`executor.rs:381` propagates `?`). Sub-batch ops are flattened
and run in the same loop — the first failure propagates up as today.

### O3 — `$query` refs from inner ops to outer aliases

Should an inner entry's `$query` ref be allowed to point to an outer
alias? E.g. inner `"orders"` references `@user` which is a sibling of
the sub-batch container.

**Options:**
- **A) Allow.** The sub-batch's inner ops are flattened into the outer
  DAG, so the dependency is valid. This is the natural consequence of
  flattening.
- **B) Forbid.** Inner ops can only reference siblings within the same
  sub-batch. Enforced at deserialization / flattening time.

**Recommendation:** Option A. Flattening makes this work for free. The
inner→outer dependency is just another edge in the DAG. Forbidding it
would require a validation pass that inspects namespace boundaries —
complexity for no clear gain. The semantic is intuitive: "I can
reference any alias visible in my scope."

### O4 — Sub-batch inside a sub-batch (recursive nesting)

The flattening algorithm is naturally recursive — a `BatchOp::Batch`
inside another `BatchOp::Batch` flattens by recursing. No special-case
code needed beyond the recursion itself and the `max_nesting_depth`
check (O1).

### O5 — `return_result: false` on the sub-batch container

If the outer caller marks the sub-batch entry as `return_result: false`
(via `op_silent`), the sub-batch's results are computed (inner ops
still run) but omitted from the final response. This is consistent with
the existing silent-entry behaviour. No new semantics needed.

### O6 — `BatchOp::Batch` in the `is_admin()` / `table_ref()` match

`is_admin()` returns `false` for `BatchOp::Batch`.
`table_ref()` returns `None`. Both are correct — the sub-batch is a
container, not an admin or data op. After flattening, these methods
are called on the expanded inner entries, not the container.

---

## §9 — Implementation sketch (file-by-file)

| File | Change |
|------|--------|
| `shamir-query-types/src/batch/types.rs` | Add `Batch` variant to `BatchOp` (struct with `queries`, `return_all`, `return_only`). Add `"queries"` key to dispatch. Update `Serialize` / `Deserialize` / `table_ref` / `is_admin`. |
| `shamir-query-types/src/batch/planner.rs` | No change (operates on the flattened map). |
| `shamir-engine/src/query/batch/executor.rs` | Add `flatten_batch` pre-step before `BatchPlanner::plan`. Add synthesise step after execution: aggregate inner results into the container's `QueryResult`. |
| `shamir-query-builder/src/batch/mod.rs` | Add `Batch::batch()` returning `SubBatchHandle`. Add `SubBatchHandle` struct with `all()` / `result()` / `first()`. |
| `shamir-client-ts/src/core/builders/batch.ts` | Add `addBatch(alias, inner)`. |
| `shamir-client-ts/src/core/builders/filter.ts` | No change (already supports dotted aliases via `queryRef`). |

**Estimated scope:** ~300–400 lines of new code (types + flattening +
builder + tests), touching 5 files. No executor refactor, no new
crates, no macro changes.
