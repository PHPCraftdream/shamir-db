# `shamir-query-builder` — design

**Status:** design / proposed (session task #151).

A client-side, fluent builder for assembling a `BatchRequest` and consuming a
`BatchResponse`. Inspired by **CodeIgniter Active Record** for single-query
ergonomics, extended with first-class **inter-query dependencies**, **function
calls** (filters / projections / computed writes), and the full **batch**
surface (transactions, isolation, durability, return policy, limits).

---

## 0. Guiding principle — thin layer, zero new wire types

Everything the builder produces already has a home in `shamir-query-types`:
`BatchRequest`, `QueryEntry`, `BatchOp`, `ReadQuery`, `Select`/`SelectItem`,
`Filter`, `FilterValue`, `GroupBy`, `OrderBy`, `Pagination`, and the write ops
(`InsertOp`/`UpdateOp`/`SetOp`/`DeleteOp`). **The builder constructs those DTOs
and nothing else** — it never invents a parallel serializable model, so what
you build is exactly what goes on the wire, and the existing planner / engine
handle it unchanged.

Consequences:
- The crate depends only on `shamir-query-types` + `serde`/`serde_json`. No
  engine, no tokio → it compiles to **WASM** for browser clients.
- `shamir-client` re-exports it and adds the async `execute(batch)` call.
- Bugs can only be ergonomics bugs, never wire-divergence bugs.

---

## 1. The universal expression type — `FilterValue` (the crux)

Filters, function arguments, and computed write-values all reduce to **one**
existing type, `FilterValue`:

| Variant | Wire shape (MessagePack map) | Meaning |
|---|---|---|
| `Null/Bool/Int/Float/String/Binary` | literal | a literal value |
| `Array(Vec<FilterValue>)` | `[...]` | a list |
| `FieldRef{path}` | `{"$ref":["a","b"]}` | this record's field |
| `QueryRef{alias,path}` | `{"$query":"@users","path":"[].id"}` | another query's result → **dependency** |
| `FnCall{call}` | `{"$fn":{"name":"strings/lower","args":[…]}}` | a funclib scalar call |

Because this single type drives **every** value slot, the builder exposes one
small constructor vocabulary (a `val` module), reused everywhere:

```rust
use shamir_query_builder::val::*;

lit(42)                       // FilterValue::Int(42)  (via From)
lit("alice")                  // FilterValue::String
col("email")                  // FilterValue::FieldRef(["email"])
col(["address","zip"])        // nested path
func("strings/lower", [col("email")])              // FilterValue::FnCall
func("math/round", [col("price"), lit(2)])
qref("users", "[].id")        // FilterValue::QueryRef  (raw form)
```

`lit` is mostly implicit via `impl From<i64|f64|bool|&str|String|Vec<u8>>`, so
`where_eq("status", "active")` needs no wrapper.

> **Why this matters:** the user's four concerns (functions in selects /
> filters / sets, plus cross-query refs) are *the same problem* — building a
> `FilterValue`. Solve it once.

---

## 2. Single query — CodeIgniter Active Record style

`Query::from(table)` returns a fluent builder that produces a `ReadQuery`.

```rust
use shamir_query_builder::{Query, val::*};

let q = Query::from("users")
    .select(["id", "name", "age"])          // SelectItem::Field ×3
    .where_eq("status", "active")           // Filter::Eq  (AND-combined)
    .where_gt("age", 18)
    .where_in("role", ["admin", "mod"])     // Filter::In
    .like("name", "Al%")                    // Filter::Like
    .order_by_desc("age")
    .limit(20)
    .offset(40);
```

### 2.1 CodeIgniter → builder mapping

| CodeIgniter AR | builder |
|---|---|
| `select('a,b')` | `.select(["a","b"])` |
| `from('t')` | `Query::from("t")` (or `.with_repo("main","t")`) |
| `where('x', 1)` | `.where_eq("x", 1)` |
| `or_where('x', 1)` | `.or_where_eq("x", 1)` |
| `where_in('x', […])` | `.where_in("x", […])` / `.where_not_in` |
| `like('n','a','after')` | `.like("n","a%")` / `.ilike` |
| `group_by('c')` | `.group_by("c")` |
| `having('cnt >', 3)` | `.having(gt("cnt", 3))` |
| `order_by('a','desc')` | `.order_by_desc("a")` / `.order_by_asc` |
| `limit(n, off)` | `.limit(n).offset(off)` |
| `group_start()…group_end()` | `.where_group(\|g\| …)` (closure) |
| `get()` | (implicit — builder *is* the query) |

### 2.2 Filter composition — AND by default, OR + nested groups explicit

Chained `.where_*` calls AND-combine (CI semantics). For OR and parentheses:

```rust
// (status = 'active') AND (role = 'admin' OR vip = true)
Query::from("users")
    .where_eq("status", "active")
    .where_group_or(|g| g                  // an OR group
        .where_eq("role", "admin")
        .where_eq("vip", true));
```

Two equivalent surfaces, pick per taste:
- **Chained** (CI-like): `.where_eq`, `.or_where_eq`, `.where_group(|g| …)`,
  `.where_group_or(|g| …)`.
- **Expression** (drop in a fully-built `Filter`): `.where_(filter)` where
  `filter` comes from free constructors + combinators:

```rust
use shamir_query_builder::filter::*;
let f = eq("status","active").and( or([eq("role","admin"), eq("vip",true)]) );
Query::from("users").where_(f);
```

Both compile to the same `Filter` tree (`And`/`Or`/`Not` + leaves). Full leaf
coverage: `eq ne gt gte lt lte in_ not_in like ilike regex between contains
contains_any contains_all is_null is_not_null exists not_exists fts
vector_similarity computed`.

---

## 3. Functions — in projection, in filters, in writes

All three are the same `FilterValue::FnCall` (or `SelectItem::Function` /
`AggregateFn` for projection), constructed via `func(...)` / `agg(...)`.

### 3.1 Scalar function in SELECT (`SelectItem::Function`)

```rust
use shamir_query_builder::{Query, sel};
Query::from("users").select([
    sel::field("name"),
    sel::func("up", "strings/upper", [col("name")]),     // → "up": UPPER(name)
    sel::func("greeting", "strings/concat",
              [func("strings/upper", [col("name")]), lit("!")]),  // nested
]);
```

### 3.2 Aggregates in SELECT

```rust
Query::from("orders")
    .select([
        sel::field("city"),
        sel::count_all("n"),                       // SelectItem::CountAll
        sel::agg(AggFunc::Sum, "amount", "total"), // builtin fast-path
        sel::agg_fn("median", "amount", "med"),    // funclib AggregateFn
    ])
    .group_by("city")
    .having(gt("n", 10));
```

### 3.3 Function as a filter value (`FilterValue::FnCall`)

```rust
// WHERE name = strings/lower("ALICE")
Query::from("users").where_eq("name", func("strings/lower", [lit("ALICE")]));
```

### 3.4 Computed values on write (`{"$fn":…}` in the record)

The write value is a MessagePack map whose field values are `FilterValue`s
(literals, `$ref`, nested `$fn`). The `Doc` builder assembles it:

```rust
use shamir_query_builder::{Insert, doc, val::*};
Insert::into("users").row(
    doc()
        .set("email", "Alice@X.COM")
        .set_expr("email_norm", func("strings/lower", [col("email")]))
);
// → values: [{ "email":"Alice@X.COM",
//              "email_norm": {"$fn":{"name":"strings/lower",
//                                    "args":[{"$ref":["email"]}]}} }]
```

`Doc::set(k, lit)` for literals, `Doc::set_expr(k, FilterValue)` for any
expression (computed `$fn`, `$ref`, or a `$query` cross-ref — §4).

---

## 4. Inter-query dependencies — typed handles over `$query`

Dependencies are **implicit on the wire** (the planner derives them from
`$query` refs). The builder makes them **typed and typo-proof** with handles,
while emitting exactly those `$query` refs.

```rust
use shamir_query_builder::{Batch, Query, val::*};

let mut b = Batch::new("orders_for_active_users");

// Adding a query returns a handle carrying its alias.
let users = b.query("users",
    Query::from("users").where_eq("active", true).select(["id"]));

// Referencing the handle injects a $query ref → the dependency is recorded
// implicitly (planner extracts it), and the builder validates the alias.
let _orders = b.query("orders",
    Query::from("orders").where_in("user_id", users.column("id")));

let req: BatchRequest = b.build();
```

### 4.1 Handle reference API → `$query` path mini-language

| builder | `$query` path | use |
|---|---|---|
| `users.column("id")` | `"[].id"` | a **column** of values (for `where_in`) |
| `users.row(0)` | `"[0]"` | a whole row |
| `users.row(0).field("id")` | `"[0].id"` | one **scalar** value (for `where_eq`) |
| `users.first().field("id")` | `"[0].id"` | alias for `row(0)` |
| `users.all()` | (no path) | the entire result |

Each returns a `FilterValue::QueryRef { alias: "@users", path }`. Because the
handle owns the alias string, you cannot misspell it, and renaming is a
one-liner. (A raw `qref("users","[].id")` escape hatch stays for dynamic
aliases.)

### 4.2 Dependencies in writes, too

`$query` refs work in `insert`/`set`/`update` values (the planner recurses
JSON), so handles compose into `Doc`:

```rust
let user = b.query("u", Query::from("users").where_eq("email", "a@x").select(["id"]));
b.insert("o", Insert::into("orders").row(
    doc().set("total", 100)
         .set_expr("user_id", user.row(0).field("id"))   // $query in a write
));
```

### 4.3 What the builder validates client-side (fail-fast, before the wire)

- **Unknown alias**: referencing a handle not added to *this* batch → compile
  pattern makes this hard, but a raw `qref` to a missing alias is caught at
  `build()`.
- **Self-reference / obvious cycle**: a query referencing itself.
- (Full cycle / depth detection stays server-side in the planner — the builder
  doesn't duplicate it, just catches the cheap mistakes early.)

---

## 5. Batch assembly — the full surface

```rust
use shamir_query_builder::{Batch, Isolation, Durability};

let req = Batch::new("checkout")
    .transactional()                       // transactional = true
    .isolation(Isolation::Serializable)    // SSI
    .durability(Durability::Synced)        // fsync before ack
    .return_only(["receipt"])              // response trims to these aliases
    .limits(BatchLimits::default())        // optional override
    // entries (each returns a handle):
    .query("cart", Query::from("cart").where_eq("user", 7))
    .query_silent("tmp", Query::from("…"))         // return_result = false
    .insert("receipt", Insert::into("receipts").row(doc()...))
    .update("u", Update::table("users").where_eq("id",7).set(doc()...))
    .delete("d", Delete::from("cart").where_eq("user",7))
    .op("mk", BatchOp::CreateIndex(/* … */))       // escape hatch for admin/rare ops
    .build();                                       // → BatchRequest
```

Mapping to `BatchRequest`:
- `.query / .insert / .update / .upsert / .delete` → `queries[alias] =
  QueryEntry{ op, return_result:true }`, return a `Handle`.
- `.query_silent(…)` / `.silent()` modifier → `return_result:false`
  (intermediate queries).
- `.transactional()/.isolation()/.durability()/.return_all()/.return_only()/
  .limits()/.name()` → the matching `BatchRequest` fields.
- `.op(alias, BatchOp)` → raw escape hatch (admin DDL, migrations, auth — typed
  helpers for these are a later phase; MVP covers read + write + deps).
- `id` is auto-generated (monotone per builder) or `.id(value)`.

---

## 6. Response handling — typed extraction

`Batch::build()` pairs with a client call returning `BatchResponse`. The
builder crate provides response helpers (pure, reusable by any transport):

```rust
let resp = client.execute(req).await?;          // shamir-client

// by alias string:
let raw: &QueryResult = resp.result("cart")?;
let rows: Vec<CartLine> = resp.rows_as("cart")?;       // serde from records
let one:  CartLine      = resp.row_as("cart", 0)?;

// by handle (carries the alias — no stringly typing):
let rows: Vec<CartLine> = resp.get_as(&cart_handle)?;

// execution plan (debugging / introspection):
let plan: &[Vec<String>] = resp.execution_plan();      // [["users"],["orders"]]

// transaction outcome (transactional batches):
if let Some(tx) = resp.transaction() {
    assert!(tx.status == "committed");                 // else tx.reason set
}
```

**Error model (confirmed from the wire):** `BatchResponse` is results-centric —
`results: {alias → QueryResult}`, `execution_plan`, `execution_time_us`,
`transaction: Option<TransactionInfo>`. There is **no per-alias error map**:

- A failing op makes the whole call return `Err(BatchError)` (execution is
  `Result<BatchResponse, BatchError>`), so op-level failures are handled at the
  `execute(..)?` boundary, not per alias.
- A transactional batch that aborts comes back as a `BatchResponse` whose
  `transaction.status == "aborted"` with `transaction.reason` (e.g.
  `"tx_conflict"`). Helper: `resp.is_committed()` / `resp.abort_reason()`.

So the builder's response helpers are: `result/rows_as/row_as/get_as`,
`execution_plan()`, `transaction()`, `is_committed()`, `abort_reason()`.

---

## 7. Crate layout

```
crates/shamir-query-builder/
├── Cargo.toml            # deps: shamir-query-types, serde, serde_json
└── src/
    ├── lib.rs            # re-exports
    ├── val.rs            # lit/col/func/qref + From impls  → FilterValue
    ├── filter.rs         # eq/ne/…/and/or/not             → Filter
    ├── query.rs          # Query (ReadQuery builder)
    ├── select.rs         # sel::field/func/agg/agg_fn/count_all → SelectItem
    ├── write.rs          # Insert/Update/Upsert/Delete + Doc
    ├── batch.rs          # Batch + Handle/RowRef
    ├── response.rs       # BatchResponse extension helpers
    └── tests/            # one file per module (per repo test layout)
```

`shamir-client` adds `Client::execute(BatchRequest) -> BatchResponse` and
re-exports the builder so app code has a single import.

---

## 8. Phased plan

- **P0 — `val` + `filter`:** the `FilterValue` constructors and `Filter`
  combinators (the foundation everything else builds on). Tests assert
  builder→MessagePack equals the hand-written wire shape.
- **P1 — `Query`:** read builder (select/where/group/having/order/limit/
  offset/pagination/distinct/count_total) → `ReadQuery`.
- **P2 — `select`:** field / func / agg / agg_fn / count_all projection items.
- **P3 — `write` + `Doc`:** Insert/Update/Upsert/Delete + computed `$fn` /
  `$ref` / `$query` field values.
- **P4 — `Batch` + handles:** assembly, `Handle`/`RowRef`, `$query` injection,
  client-side alias validation, return policy / tx / isolation / durability.
- **P5 — `response`:** typed extraction + error surface (after confirming the
  response error shape).
- **P6 — client wiring + test migration (#151):** `Client::execute`, then port
  the existing query tests (`serde_json::json!` literals) onto the builder.
- **P7 (later) — typed admin/auth/migration ops** beyond the `.op()` escape
  hatch.

---

## 9. Decisions (resolved)

1. **Filter surface — BOTH.** CI-chained API (`.where_eq`/`.or_where`/
   `.where_group`) is the headline; the expression DSL (`eq(..).and(..)` +
   `.where_(f)`) is the escape hatch and powers `having`. Both compile to one
   `Filter` tree.
2. **Dependency API — typed handles.** `b.query(..) -> Handle`;
   `handle.column/row/field/all` emit `$query` refs. Raw `qref("alias","path")`
   stays as an escape hatch for dynamic aliases.
3. **Crate — standalone `shamir-query-builder`** (depends only on
   `shamir-query-types` + serde; WASM-friendly). `shamir-client` re-exports it.
4. **Response errors — resolved (see §6):** no per-alias error map; op failures
   surface as `Err(BatchError)` from `execute`, tx aborts via
   `transaction.reason`. Helpers: `is_committed()` / `abort_reason()`.
