בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# TypeScript Query Builder — Capability Coverage Audit

**Scope**: `crates/shamir-client-ts/src/core/builders/` (10 builder files) + `crates/shamir-client-ts/src/core/types/` (10 type files), measured against the Rust builder (`crates/shamir-query-builder/src/`) and the wire protocol ceiling (`crates/shamir-query-types/src/`).

**Date**: 2025-06-24 · **Method**: read-only file enumeration, zero-trust citation.

---

## How to read this document

- ✅ = the TS builder has a public constructor/method that produces the exact wire shape, and the wire type is declared.
- 🟡 = partial: the wire type exists in TS but either (a) the builder lacks ergonomic support for some sub-field, or (b) the TS uses `unknown` / a looser type than Rust.
- ❌ = no TS constructor and/or no TS wire type for a capability the Rust builder or wire protocol exposes.

All file paths are relative to the repo root. "Wire" = `shamir-query-types`. "Rust builder" = `shamir-query-builder`. "TS" = `shamir-client-ts`.

---

## 1. OQL Coverage (Read / Filter / Value / Select)

### 1.1 ReadQuery — `query.ts`

Wire source: `shamir-query-types/src/read/read_query.rs`. Rust builder: `shamir-query-builder/src/query/query.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 1 | `Query::from(table)` (default repo) | ✅ `Query::from` | ✅ `Query.from()` | `builders/query.ts:79` |
| 2 | `Query::with_repo(repo, table)` | ✅ `Query::with_repo` | ✅ `Query.withRepo()` | `builders/query.ts:84` |
| 3 | `.select(items)` (projection set) | ✅ `Query::select` | ✅ `Query.select()` | `builders/query.ts:94` |
| 4 | `SELECT *` (all) | ✅ `Select::all()` default | ✅ `Query.selectAll()` | `builders/query.ts:102` |
| 5 | `DISTINCT` | ✅ `.distinct()` | ✅ `Query.distinct()` | `builders/query.ts:108` |
| 6 | `.where(Filter)` (drop-in) | ✅ `Query::where_` (macro) | ✅ `Query.where()` | `builders/query.ts:116` |
| 7 | `.andWhere(Filter)` (AND-combine) | ✅ `FilterExt::and` | ✅ `Query.andWhere()` | `builders/query.ts:122` |
| 8 | Inline `where_eq/gt/...` (AND-combine leaf) | ✅ `where_methods!` macro (24+ methods) | ❌ **Missing** | TS has no inline `whereEq`/`whereGt` etc.; user must build a `Filter` then call `.where()`. See gap G1. |
| 9 | Inline `or_where_eq/gt/...` (OR-combine leaf) | ✅ `where_methods!` OR section | ❌ **Missing** | Same as above — no `orWhereEq` etc. |
| 10 | `where_group(closure)` (nested AND group) | ✅ `Conds::where_group` | ❌ **Missing** | No closure-based nested group builder. User must hand-build `and([...])`. |
| 11 | `where_group_or(closure)` (nested OR group) | ✅ `Conds::where_group_or` | ❌ **Missing** | Same — no `orWhereGroup`. |
| 12 | `.groupBy(fields)` | ✅ `Query::group_by` / `group_by_many` | ✅ `Query.groupBy()` | `builders/query.ts:131` (variadic) |
| 13 | `.having(Filter)` | ✅ `Query::having` | ✅ `Query.having()` | `builders/query.ts:137` |
| 14 | `.orderByAsc(field)` | ✅ `Query::order_by_asc` | ✅ `Query.orderByAsc()` | `builders/query.ts:152` — **bonus**: TS also accepts `nulls` ordering param; Rust requires `.order_by(OrderByItem)`. |
| 15 | `.orderByDesc(field)` | ✅ `Query::order_by_desc` | ✅ `Query.orderByDesc()` | `builders/query.ts:157` — same bonus. |
| 16 | `.orderBy(OrderByItem)` (full item) | ✅ `Query::order_by` | ✅ `Query.orderBy()` | `builders/query.ts:145` |
| 17 | `.limit(n)` | ✅ `Query::limit` | ✅ `Query.limit()` | `builders/query.ts:175` |
| 18 | `.offset(n)` | ✅ `Query::offset` | ✅ `Query.offset()` | `builders/query.ts:182` |
| 19 | `.page(page, size)` | ✅ `Query::page` | ✅ `Query.page()` | `builders/query.ts:189` |
| 20 | `.count_total(bool)` | ✅ `Query::count_total` | ✅ `Query.countTotal()` | `builders/query.ts:197` |
| 21 | `.as_of_version(v)` | ✅ `Query::as_of_version` | ✅ `Query.asOfVersion()` | `builders/query.ts:205` |
| 22 | `.as_of_timestamp(ms)` | ✅ `Query::as_of_timestamp` | ✅ `Query.asOfTimestamp()` | `builders/query.ts:211` |
| 23 | `.as_of(At)` (generic) | ✅ (via type) | ✅ `Query.asOf()` | `builders/query.ts:217` |
| 24 | `.history()` (full scan) | ✅ `Query::history` | 🟡 **Partial** | TS `Query.history()` (`builders/query.ts:227`) accepts opts `{from,to,limit,order}` — covers `history_range` too, but always requires an explicit call with defaults. The bare no-arg Rust `.history()` shortcut is available by passing `{}`. Functionally equivalent; marked 🟡 only because the ergonomics differ. |
| 25 | `.history_range(from,to,limit,order)` | ✅ `Query::history_range` | ✅ `Query.history(opts)` | `builders/query.ts:227` — merged into one method. |
| 26 | `.with_version()` | ✅ `Query::with_version` | ✅ `Query.withVersion()` | `builders/query.ts:242` |
| 27 | `.build()` → `ReadQuery` | ✅ `Query::build` | ✅ `Query.build()` | `builders/query.ts:250` |

### 1.2 SelectItem constructors — `select.ts`

Wire source: `shamir-query-types/src/read/select.rs`. Rust builder: `shamir-query-builder/src/select/select_item.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 28 | `all()` (`SELECT *`) | ✅ `select::all` | ✅ `select.all()` | `builders/select.ts:30` |
| 29 | `field(path)` | ✅ `select::field` | ✅ `select.field()` | `builders/select.ts:35` |
| 30 | `field_as(path, alias)` | ✅ `select::field_as` | ✅ `select.field(spec, alias)` | `builders/select.ts:35` — alias is an optional 2nd arg (merged). |
| 31 | `count_all(alias)` | ✅ `select::count_all` | ✅ `select.countAll()` | `builders/select.ts:42` |
| 32 | `agg(func, field, alias)` generic | ✅ `select::agg` | ✅ `select.aggregate()` | `builders/select.ts:52` — TS adds `distinct` opt; Rust uses separate `agg_distinct`. |
| 33 | `agg_distinct(func, field, alias)` | ✅ `select::agg_distinct` | ✅ via `{distinct:true}` | `builders/select.ts:52` opts.distinct |
| 34 | `sum(field, alias)` | ✅ `select::sum` | ✅ `select.sum()` | `builders/select.ts:76` |
| 35 | `avg(field, alias)` | ✅ `select::avg` | ✅ `select.avg()` | `builders/select.ts:84` |
| 36 | `min(field, alias)` | ✅ `select::min` | ✅ `select.min()` | `builders/select.ts:92` |
| 37 | `max(field, alias)` | ✅ `select::max` | ✅ `select.max()` | `builders/select.ts:100` |
| 38 | `count(field, alias)` | ✅ `select::count` | ✅ `select.count()` | `builders/select.ts:68` |
| 39 | `agg_fn(name, field, alias)` (funclib agg) | ✅ `select::agg_fn` | ✅ `select.aggregateFn()` | `builders/select.ts:112` |
| 40 | `agg_fn_distinct(name, field, alias)` | ✅ `select::agg_fn_distinct` | ✅ via `{distinct:true}` | `builders/select.ts:112` opts.distinct |
| 41 | `func(alias, name, args)` (scalar fn) | ✅ `select::func` | ✅ `select.func()` | `builders/select.ts:132` — TS signature is `(name, args, alias)`; alias is last & optional. |
| 42 | `SelectItem::Expression` (computed `expr`) | ✅ wire enum exists (`select.rs:110`) | 🟡 **Partial** | Wire type `{type:'expr'; expr:unknown}` exists in `types/query.ts:64` but TS marks `expr` as `unknown` — there is **no TS builder** for `SelectExpr` (Add/Sub/Mul/Div/Field/Literal). The shape is pass-through only. Rust builder has no constructor either (commented "future: computed fields"), so this is a wire-level gap, not a builder parity gap. |

### 1.3 Filter leaf constructors — `filter.ts`

Wire source: `shamir-query-types/src/filter/filter_enum.rs`. Rust builder: `shamir-query-builder/src/filter/leaf.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 43 | `eq` | ✅ | ✅ `filter.eq()` | `builders/filter.ts:29` |
| 44 | `ne` | ✅ | ✅ `filter.ne()` | `builders/filter.ts:34` |
| 45 | `gt` | ✅ | ✅ `filter.gt()` | `builders/filter.ts:39` |
| 46 | `gte` | ✅ | ✅ `filter.gte()` | `builders/filter.ts:44` |
| 47 | `lt` | ✅ | ✅ `filter.lt()` | `builders/filter.ts:49` |
| 48 | `lte` | ✅ | ✅ `filter.lte()` | `builders/filter.ts:54` |
| 49 | `field_eq` (`op:"field"`) | ✅ | ✅ `filter.fieldEq()` | `builders/filter.ts:64` |
| 50 | `in_` | ✅ | ✅ `filter.in_()` | `builders/filter.ts:71` |
| 51 | `not_in` | ✅ | ✅ `filter.notIn()` | `builders/filter.ts:76` |
| 52 | `like` | ✅ | ✅ `filter.like()` | `builders/filter.ts:83` |
| 53 | `ilike` | ✅ | ✅ `filter.ilike()` | `builders/filter.ts:88` |
| 54 | `regex` | ✅ | ✅ `filter.regex()` | `builders/filter.ts:93` |
| 55 | `is_null` | ✅ | ✅ `filter.isNull()` | `builders/filter.ts:100` |
| 56 | `is_not_null` | ✅ | ✅ `filter.isNotNull()` | `builders/filter.ts:105` |
| 57 | `exists` | ✅ | ✅ `filter.exists()` | `builders/filter.ts:110` |
| 58 | `not_exists` | ✅ | ✅ `filter.notExists()` | `builders/filter.ts:115` |
| 59 | `contains` | ✅ | ✅ `filter.contains()` | `builders/filter.ts:122` |
| 60 | `contains_any` | ✅ | ✅ `filter.containsAny()` | `builders/filter.ts:127` |
| 61 | `contains_all` | ✅ | ✅ `filter.containsAll()` | `builders/filter.ts:135` |
| 62 | `between` | ✅ | ✅ `filter.between()` | `builders/filter.ts:145` |
| 63 | `fts(field, query, mode)` | ✅ | ✅ `filter.fts()` | `builders/filter.ts:159` |
| 64 | `vector_similarity(field, query, k)` | ✅ | ✅ `filter.vectorSimilarity()` | `builders/filter.ts:170` |
| 65 | `computed(expr_op, field, cmp, value)` | ✅ | ✅ `filter.computed()` | `builders/filter.ts:185` |
| 66 | `computed_with_args(expr_op, field, args, cmp, value)` | ✅ `leaf.rs:251` | ✅ via optional `exprArgs` param | `builders/filter.ts:185` — 5th param `exprArgs?`. **TS is more ergonomic** (one function). |
| 67 | `and(filters)` | ✅ | ✅ `filter.and()` | `builders/filter.ts:277` — supports both `(a,b)` and `([...])` overloads. |
| 68 | `or(filters)` | ✅ | ✅ `filter.or()` | `builders/filter.ts:294` — same dual overload. |
| 69 | `not(filter)` | ✅ | ✅ `filter.not()` | `builders/filter.ts:308` |
| 70 | `FilterExt` trait (`.and()`/`.or()`/`.negate()` on Filter) | ✅ `combinators.rs:35` | ❌ **Missing** | TS `Filter` is a plain union type — no chainable methods. User must call the free `and()`/`or()`/`not()` functions. Acceptable in TS idiom; see gap G2. |

### 1.4 FilterValue constructors — `filter.ts` (value section)

Wire source: `shamir-query-types/src/filter/filter_value.rs`. Rust builder: `shamir-query-builder/src/val/filter_value.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 71 | `lit(v)` (literal passthrough) | ✅ `val::lit` | ✅ implicit (JS literals) | TS uses native JS values directly as `FilterValue`; no explicit `lit()` needed. |
| 72 | `lit_u64(v)` (u64 escape hatch) | ✅ `val::lit_u64` | ❌ **Missing** | TS `number` handles u53 safely; for full u64 range the TS client would need `bigint`. No constructor. Low priority (wire value is `i64` anyway). |
| 73 | `bin(bytes)` (`Binary`) | ✅ `val::bin` | 🟡 **Partial** | `FilterValue` type includes `Uint8Array` (`types/filter.ts:57`), but there is **no `bin()` constructor**. User must pass a raw `Uint8Array`. |
| 74 | `null()` (`Null`) | ✅ `val::null` | ✅ implicit (`null` literal) | TS `FilterValue` includes `null`. |
| 75 | `col(path)` (`FieldRef` / `$ref`) | ✅ `val::col` | ✅ `filter.ref()` | `builders/filter.ts:232` |
| 76 | `func(name, args)` (`FnCall` / `$fn`) | ✅ `val::func` | ✅ `filter.fn()` | `builders/filter.ts:245` — handles both Simple (no args) and Complex (with args) variants. |
| 77 | `param(name)` (`Param` / `$param`) | ✅ `val::param` | ✅ `filter.param()` | `builders/filter.ts:210` |
| 78 | `qref(alias, path)` (`QueryRef` / `$query`) | ✅ `val::qref` | ✅ `filter.queryRef()` | `builders/filter.ts:222` |
| 79 | `qref_all(alias)` (`QueryRef` no path) | ✅ `val::qref_all` | ✅ `filter.queryRef(alias)` (path omitted) | `builders/filter.ts:222` — `path` is optional. |
| 80 | `expr(op, args)` (`Expr` / `$expr`) | ✅ wire enum exists | ✅ `filter.expr()` | `builders/filter.ts:257` — Rust builder has no constructor (wire-level only), TS **exceeds** Rust here. |
| 81 | `cond(if, then, else)` (`Cond` / `$cond`) | ✅ wire enum exists | ✅ `filter.cond()` | `builders/filter.ts:266` — same, TS exceeds Rust builder. |

### 1.5 Batch dependency Handle — `batch.ts` vs `batch/handle.rs`

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 82 | `Handle::column(field)` → `[].field` path | ✅ `handle.rs:25` | 🟡 **Partial** | TS has no `Handle` type. `Batch.add()` returns `this` (the batch), not a handle. Users build `$query` refs manually via `filter.queryRef(alias, path)`. See gap G3. |
| 83 | `Handle::row(index)` → `[index]` | ✅ `handle.rs:33` | ❌ **Missing** | No typed row ref. |
| 84 | `Handle::first()` → `[0]` | ✅ `handle.rs:41` | ❌ **Missing** | No typed first-row ref. |
| 85 | `Handle::all()` → entire result | ✅ `handle.rs:46` | ❌ **Missing** | No typed "all" ref. |
| 86 | `RowRef::field(field)` → `[i].field` | ✅ `handle.rs:62` | ❌ **Missing** | No `RowRef` type. |
| 87 | `RowRef::get()` → `[i]` | ✅ `handle.rs:72` | ❌ **Missing** | — |
| 88 | `Batch::after(dependent, on)` (ordering edge) | ✅ `batch.rs:692` | 🟡 **Partial** | TS `Batch.add()` accepts `opts.after: string[]` (`builders/batch.ts:82`) but there is **no `Batch.after(h1, h2)` method** to wire it declaratively. |

### 1.6 Batch orchestration — `batch.ts`

Wire source: `shamir-query-types/src/batch/batch_request.rs`. Rust builder: `shamir-query-builder/src/batch/batch.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 89 | `Batch::create(id)` / `new()` | ✅ `Batch::new` | ✅ `Batch.create()` | `builders/batch.ts:64` |
| 90 | `Batch::named(name)` | ✅ `Batch::named` | ✅ `Batch.name()` | `builders/batch.ts:151` |
| 91 | `.id(v)` | ✅ `Batch::id` | ✅ `Batch.create(id)` | TS passes id to constructor. |
| 92 | `.transactional()` | ✅ `Batch::transactional` | ✅ `Batch.transactional()` | `builders/batch.ts:161` |
| 93 | `.isolation(level)` | ✅ `Batch::isolation` | ✅ `Batch.transactional(iso)` | `builders/batch.ts:161` — merged into transactional. |
| 94 | `.durability(level)` | ✅ `Batch::durability` | ✅ `Batch.durability()` | `builders/batch.ts:168` |
| 95 | `.return_all(bool)` | ✅ `Batch::return_all` | ✅ `Batch.returnAll()` | `builders/batch.ts:177` |
| 96 | `.return_flagged()` (return only `return_result:true`) | ✅ `Batch::return_flagged` | 🟡 **Partial** | TS `returnAll(false)` achieves `return_all=false` but there's no dedicated "flagged-only" mode distinct from `returnOnly`. |
| 97 | `.return_only(aliases)` | ✅ `Batch::return_only` | ✅ `Batch.returnOnly()` | `builders/batch.ts:183` |
| 98 | `.limits(BatchLimits)` | ✅ `Batch::limits` | ✅ `Batch.limits()` | `builders/batch.ts:192` — TS fills defaults from `DEFAULT_LIMITS`. |
| 99 | `.add(alias, op)` (generic) | ✅ `Batch::op` | ✅ `Batch.add()` | `builders/batch.ts:79` — TS auto-calls `.build()` if the op is a builder. |
| 100 | Silent add (`return_result:false`) | ✅ `Batch::op_silent` | ✅ `Batch.add(..., {returnResult:false})` | `builders/batch.ts:91` |
| 101 | Typed `.query(alias, q)` | ✅ `Batch::query` | ✅ via `Batch.add(alias, query)` | TS is generic; Rust has typed shortcuts. |
| 102 | Silent query | ✅ `Batch::query_silent` | ✅ via `{returnResult:false}` | — |
| 103 | `.sub_batch(alias, inner, bind)` | ✅ `Batch::sub_batch` | ✅ `Batch.subBatch()` | `builders/batch.ts:115` — TS auto-builds inner `Batch`. |
| 104 | `.sub_batch_no_bind(alias, inner)` | ✅ `Batch::sub_batch_no_bind` | ✅ `Batch.subBatch()` (bind omitted) | `builders/batch.ts:115` — `bind` is optional. |
| 105 | `.subscribe(alias, sub)` | ✅ `Batch::subscribe` | ✅ `Batch.subscribe()` | `builders/batch.ts:245` — **bonus**: TS builds the `SubscribeOp` from user-friendly config. |
| 106 | `.unsubscribe(alias, id)` | ✅ `Batch::unsubscribe` | ✅ `Batch.unsubscribe()` | `builders/batch.ts:256` |
| 107 | `.call(alias, name, params)` | ✅ `Batch::call` | ✅ via `Batch.add(alias, call(...))` | `builders/call.ts:19` |
| 108 | `.call_in_repo(alias, name, repo, params)` | ✅ `Batch::call_in_repo` | ✅ via `call(name, params, {repo})` | `builders/call.ts:19` — opts.repo. |
| 109 | `.try_build()` (validation) | ✅ `Batch::try_build` | ❌ **Missing** | TS `Batch.build()` does no `$query` ref or `after` validation. See gap G4. |
| 110 | `.to_msgpack()` / `.to_request_via_msgpack()` | ✅ `batch.rs:598` | ❌ **N/A** | TS uses `@msgpack/msgpack` at the transport layer, not in the builder. Not a gap — different architecture. |

### 1.7 Write operations — `write.ts`

Wire source: `shamir-query-types/src/write/types.rs`. Rust builder: `shamir-query-builder/src/write/`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 111 | `insert(table)` builder | ✅ `Insert::into` | ✅ `write.insert()` | `builders/write.ts:40` |
| 112 | `Insert::with_repo(repo, table)` | ✅ | ✅ `insert(table, {repo})` | `builders/write.ts:40` opts.repo |
| 113 | `.row(doc)` / `.rows(docs)` | ✅ `Insert::row`/`rows` | ✅ `insert(table, values[])` | `builders/write.ts:40` — accepts single or array. |
| 114 | `update(table)` builder | ✅ `Update::table` | ✅ `write.update()` | `builders/write.ts:112` |
| 115 | `Update::with_repo` | ✅ | ✅ `update(table, {repo})` | `builders/write.ts:112` |
| 116 | `.where_(filter)` | ✅ `Update::where_` | ✅ `UpdateBuilder.where()` | `builders/write.ts:68` |
| 117 | `.set(doc)` | ✅ `Update::set` | ✅ `UpdateBuilder.set()` | `builders/write.ts:74` |
| 118 | `.returning(mode)` | ✅ `Update::returning` | ✅ `UpdateBuilder.returning()` | `builders/write.ts:84` |
| 119 | `.returning_fields(mode, fields)` | ✅ `Update::returning_fields` | ✅ `UpdateBuilder.returning(mode, fields)` | `builders/write.ts:84` — merged. |
| 120 | `upsert(table)` builder | ✅ `Upsert::table` | ✅ `write.upsert()` | `builders/write.ts:122` |
| 121 | `Upsert::with_repo` | ✅ | ✅ `upsert(table, key, val, {repo})` | `builders/write.ts:122` |
| 122 | `.key(doc)` / `.value(doc)` | ✅ `Upsert::key`/`value` | ✅ `upsert(table, key, value)` | `builders/write.ts:122` — positional. |
| 123 | `delete(table)` builder | ✅ `Delete::from_table` | ✅ `write.del()` | `builders/write.ts:137` |
| 124 | `Delete::with_repo` | ✅ | ✅ `del(table, where, {repo})` | `builders/write.ts:137` |
| 125 | `.where_(filter)` (required) | ✅ `Delete::where_` | ✅ `del(table, where)` | `builders/write.ts:137` — where is required param. |
| 126 | `Doc` builder (`.set(key, val)`) | ✅ `write::Doc` | ❌ **Missing** | No TS equivalent. TS users pass plain JS objects (which is idiomatic), but **cannot embed `$ref`/`$fn` expressions in write values** without manually constructing the wire shape. See gap G5. |

### 1.8 Subscribe — `subscribe.ts`

Wire source: `shamir-query-types/src/subscribe/`. Rust builder: `shamir-query-builder/src/batch/subscribe.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 127 | `Subscribe::table(table)` | ✅ | ✅ `subscribe({store, table})` | `builders/subscribe.ts:86` |
| 128 | `Subscribe::source(src)` / `sources(srcs)` | ✅ | ✅ `subscribe(sources[])` | `builders/subscribe.ts:86` — accepts array. |
| 129 | `SourceBuilder::filter(f)` | ✅ | ✅ `source.where` | `builders/subscribe.ts:33` — accepts Filter or callback. |
| 130 | `SourceBuilder::events(mask)` | ✅ | ✅ `source.on` | `builders/subscribe.ts:35` — maps `'any'`→`'all'`. |
| 131 | `deliver_records()` | ✅ | ✅ `source.deliver: 'records'` | `builders/subscribe.ts:37` |
| 132 | `deliver_keys()` | ✅ | ✅ `source.deliver: 'keys'` | `builders/subscribe.ts:37` |
| 133 | `deliver_batch(SubBatchOp)` | ✅ | ✅ `source.handle(batch => ...)` | `builders/subscribe.ts:39` — callback-based. |
| 134 | `deliver_call(CallOp)` | ✅ | ❌ **Missing** | No TS constructor for `DeliverMode::Call`. Wire type `{call: CallOp}` exists in `types/subscribe.ts:44` but the builder cannot produce it. See gap G6. |
| 135 | `.with_initial()` | ✅ | ✅ `opts.initial` | `builders/subscribe.ts:47` |
| 136 | `.from_version(v)` | ✅ | ✅ `opts.fromVersion` | `builders/subscribe.ts:48` |
| 137 | `unsubscribeOp(subId)` | ✅ (via `Batch::unsubscribe`) | ✅ `unsubscribeOp()` | `builders/subscribe.ts:137` |

### 1.9 Call — `call.ts`

Wire source: `shamir-query-types/src/call/mod.rs`. Rust builder: `Batch::call` in `batch.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 138 | `call(name, params)` (default repo) | ✅ `Batch::call` | ✅ `call()` | `builders/call.ts:19` |
| 139 | `call_in_repo(name, repo, params)` | ✅ `Batch::call_in_repo` | ✅ `call(name, params, {repo})` | `builders/call.ts:19` |

---

## 2. DDL Coverage

Wire source: `shamir-query-types/src/admin/` + `auth/`. Rust builder: `shamir-query-builder/src/ddl/`.

### 2.1 Database / Repo / Table DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 140 | `create_db(name)` | ✅ `ddl::create_db` | ✅ `createDb()` | `builders/ddl.ts:94` — TS adds `if_not_exists`. |
| 141 | `drop_db(name)` (HMAC) | ✅ `ddl::drop_db` | ✅ `dropDb()` | `builders/ddl.ts:447` — HMAC via signer. TS adds `cascade`. |
| 142 | `create_repo(name)` | ✅ `ddl::create_repo` | ✅ `createRepo()` | `builders/ddl.ts:104` — TS adds `engine`/`path`/`tables`/`if_not_exists`. |
| 143 | `drop_repo(repo)` (HMAC) | ✅ `ddl::drop_repo` | ✅ `dropRepo()` | `builders/ddl.ts:462` — TS adds `cascade`. |
| 144 | `create_table(name)` | ✅ `ddl::create_table` | ✅ `createTable()` | `builders/ddl.ts:123` |
| 145 | Create table `.retention(r)` | ✅ `CreateTable::retention` | ✅ `createTable(name, {retention})` | `builders/ddl.ts:137` |
| 146 | Create table `.schema(rules)` | ✅ `CreateTable::schema` | ✅ `createTable(name, {schema})` | `builders/ddl.ts:138` |
| 147 | `drop_table(name)` (HMAC) | ✅ `ddl::drop_table` | ✅ `dropTable()` | `builders/ddl.ts:478` |

### 2.2 Index DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 148 | `create_index(name, table)` builder | ✅ `ddl::create_index` | ✅ `createIndex()` | `builders/ddl.ts:144` |
| 149 | `.fields(paths)` | ✅ `CreateIndex::fields` | ✅ `createIndex(name, table, fields)` | `builders/ddl.ts:147` — positional. |
| 150 | `.unique()` | ✅ | ✅ `{unique:true}` | `builders/ddl.ts:168` |
| 151 | `.sorted()` | ✅ | ✅ `{sorted:true}` | `builders/ddl.ts:168` |
| 152 | `.index_type(t)` | ✅ | ✅ `{index_type}` | `builders/ddl.ts:171` |
| 153 | `.fts_tokenizer(t)` | ✅ | ✅ `{fts_tokenizer}` | `builders/ddl.ts:172` |
| 154 | `.fts_language(l)` | ✅ | ✅ `{fts_language}` | `builders/ddl.ts:174` |
| 155 | `.functional_op(op)` | ✅ | ✅ `{functional_op}` | `builders/ddl.ts:176` |
| 156 | `.functional_args(args)` | ✅ | ✅ `{functional_args}` | `builders/ddl.ts:178` |
| 157 | `.vector_dim(d)` | ✅ | ✅ `{vector_dim}` | `builders/ddl.ts:180` |
| 158 | `.vector_metric(m)` | ✅ | ✅ `{vector_metric}` | `builders/ddl.ts:182` |
| 159 | `.include(paths)` (covering index) | ✅ | ✅ `{include}` | `builders/ddl.ts:183` |
| 160 | `.if_not_exists()` | ✅ | ✅ `{if_not_exists}` | `builders/ddl.ts:185` |
| 161 | `drop_index(...)` (HMAC) | ✅ `ddl::drop_index` | ✅ `dropIndex()` | `builders/ddl.ts:493` |

### 2.3 Schema DDL (declarative constraints)

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 162 | `field(path)` fluent builder | ✅ `ddl::field` | ✅ `ddl.field()` / `field()` | `builders/ddl.ts:673` |
| 163 | `.string()` / `.int()` / `.f64()` / `.dec()` / `.bool()` / `.bin()` / `.list()` / `.map()` / `.any()` | ✅ all | ✅ all | `builders/ddl.ts:588-596` |
| 164 | `.type_tag(tag)` | ✅ `FieldBuilder::type_tag` | ✅ `.typeTag()` | `builders/ddl.ts:597` |
| 165 | `.required()` | ✅ | ✅ | `builders/ddl.ts:600` |
| 166 | `.nullable()` | ✅ | ✅ | `builders/ddl.ts:601` |
| 167 | `.unsigned()` | ✅ | ✅ | `builders/ddl.ts:602` |
| 168 | `.min(v)` (int) | ✅ `FieldBuilder::min` | ✅ `.min()` | `builders/ddl.ts:603` — TS uses `number` (no separate `min_f64`). |
| 169 | `.min_f64(v)` | ✅ `schema.rs:129` | 🟡 **Partial** | TS `.min(number)` covers both but loses the int/f64 distinction. Wire type `NumDto = number` (`types/ddl.ts:27`) so functionally equivalent. |
| 170 | `.max(v)` / `.max_f64(v)` | ✅ | 🟡 same as min | `builders/ddl.ts:604` |
| 171 | `.len(v)` | ✅ | ✅ | `builders/ddl.ts:605` |
| 172 | `.max_len(v)` | ✅ | ✅ | `builders/ddl.ts:606` |
| 173 | `.min_len(v)` | ✅ | ✅ | `builders/ddl.ts:607` |
| 174 | `.array_of(tag)` | ✅ | ✅ | `builders/ddl.ts:608` |
| 175 | `.scalar(name)` (Phase B) | ✅ | ✅ | `builders/ddl.ts:616` |
| 176 | `.format(kind)` (Phase B) | ✅ | ✅ | `builders/ddl.ts:635` |
| 177 | `.compare(other, op)` (Phase B) | ✅ | ✅ | `builders/ddl.ts:642` |
| 178 | `.foreign_key(table, field)` (Phase C2) | ✅ | ✅ | `builders/ddl.ts:651` |
| 179 | `.unique()` (Phase C3) | ✅ | ✅ | `builders/ddl.ts:630` |
| 180 | `.one_of(values)` | ✅ wire: `ConstraintsDto.one_of` | ✅ `.oneOf()` | `builders/ddl.ts:621` |
| 181 | `set_table_schema(table)` | ✅ `ddl::set_table_schema` | ✅ `setTableSchema()` | `builders/ddl.ts:680` |
| 182 | `.expected_version(v)` | ✅ `SetTableSchemaBuilder::expected_version` | ✅ `{expectedVersion}` | `builders/ddl.ts:683` |
| 183 | `add_schema_rule(table)` | ✅ `ddl::add_schema_rule` | ✅ `addSchemaRule()` | `builders/ddl.ts:696` |
| 184 | `remove_schema_rule(table, path)` | ✅ `ddl::remove_schema_rule` | ✅ `removeSchemaRule()` | `builders/ddl.ts:709` |
| 185 | `get_table_schema(table)` | ✅ `ddl::get_table_schema` | ✅ `getTableSchema()` | `builders/ddl.ts:722` |

### 2.4 Buffer Config DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 186 | `set_buffer_config(table, config)` | ✅ | ✅ `setBufferConfig()` | `builders/ddl.ts:190` |
| 187 | `get_buffer_config(table)` | ✅ | ✅ `getBufferConfig()` | `builders/ddl.ts:202` |
| 188 | `alter_buffer_config(table, patch)` | ✅ | ✅ `alterBufferConfig()` | `builders/ddl.ts:214` |

### 2.5 Migration DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 189 | `start_migration(table, dst_repo, dst_engine)` (HMAC) | ✅ | ✅ `startMigration()` | `builders/ddl.ts:514` |
| 190 | `.dst_path(path)` | ✅ `StartMigration::dst_path` | ✅ `{dst_path}` | `builders/ddl.ts:537` |
| 191 | `commit_migration(id)` (HMAC) | ✅ | ✅ `commitMigration()` | `builders/ddl.ts:542` |
| 192 | `rollback_migration(id)` (HMAC) | ✅ | ✅ `rollbackMigration()` | `builders/ddl.ts:555` |
| 193 | `migration_status(id)` | ✅ | ✅ `migrationStatus()` | `builders/ddl.ts:227` |

### 2.6 Function DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 194 | `create_function(name)` | ✅ | ✅ `createFunction()` | `builders/ddl.ts:234` |
| 195 | `.source(s)` / `.wasm(b)` / `.replace()` | ✅ all | ✅ all opts | `builders/ddl.ts:246-248` |
| 196 | `drop_function(name)` | ✅ | ✅ `dropFunction()` | `builders/ddl.ts:252` |
| 197 | `rename_function(from, to)` | ✅ | ✅ `renameFunction()` | `builders/ddl.ts:257` |
| 198 | `create_function_folder(segs)` | ✅ | ✅ `createFunctionFolder()` | `builders/ddl.ts:349` |

### 2.7 Validator DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 199 | `create_validator(name)` | ✅ | ✅ `createValidator()` | `builders/ddl.ts:265` |
| 200 | `drop_validator(name)` | ✅ | ✅ `dropValidator()` | `builders/ddl.ts:283` |
| 201 | `rename_validator(from, to)` | ✅ | ✅ `renameValidator()` | `builders/ddl.ts:288` |
| 202 | `bind_validator(name, table)` | ✅ | ✅ `bindValidator()` | `builders/ddl.ts:296` |
| 203 | Bind `.db(d)` / `.repo(r)` / `.ops(ops)` / `.priority(p)` | ✅ all | ✅ all | `builders/ddl.ts:307-313` |
| 204 | `unbind_validator(name, table)` | ✅ | ✅ `unbindValidator()` | `builders/ddl.ts:317` |
| 205 | `list_validators(table)` | ✅ | ✅ `listValidators()` | `builders/ddl.ts:334` |

### 2.8 Retention / Purge / Changes

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 206 | `set_retention(table, r)` | ✅ | ✅ `setRetention()` | `builders/ddl.ts:356` |
| 207 | `purge_history(table, scope)` | ✅ | ✅ `purgeHistory()` | `builders/ddl.ts:369` |
| 208 | `PurgeScope::OlderThan { timestamp }` | ✅ wire enum | ✅ `olderThan()` | `builders/ddl.ts:82` |
| 209 | `PurgeScope::OlderThanAge { age_secs }` | ✅ wire enum | ✅ `olderThanAge()` | `builders/ddl.ts:87` |
| 210 | `changes_since(from)` | ✅ | ✅ `changesSince()` | `builders/ddl.ts:382` |
| 211 | Changes-since `.limit(n)` | ✅ `ChangesSince::limit` | ✅ `{limit}` | `builders/ddl.ts:390` |
| 212 | `Retention` helper: `currentOnly()` | ❌ Rust has no helper | ✅ `currentOnly()` | `builders/ddl.ts:75` — **TS exceeds Rust** (Rust users construct `Retention { max_count: Some(0), .. }` manually). |

### 2.9 List operations

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 213 | `list_databases()` | ✅ | ✅ `listDatabases()` | `builders/ddl.ts:396` |
| 214 | `list_repos()` | ✅ | ✅ `listRepos()` | `builders/ddl.ts:400` |
| 215 | `list_tables(repo)` | ✅ | ✅ `listTables()` | `builders/ddl.ts:404` |
| 216 | `list_indexes(table, repo)` | ✅ | ✅ `listIndexes()` | `builders/ddl.ts:408` |
| 217 | `list_users()` | ✅ | ✅ `listUsers()` | `builders/ddl.ts:415` |
| 218 | `list_roles()` | ✅ | ✅ `listRoles()` | `builders/ddl.ts:419` |
| 219 | `list_functions(folder)` | ✅ | ✅ `listFunctions()` | `builders/ddl.ts:423` |
| 220 | `list_all_validators()` | ✅ `ddl::list_all_validators` | ✅ `listValidators_()` | `builders/ddl.ts:431` — TS name has trailing `_` to avoid clash with per-table `listValidators()`. |
| 221 | `list_function_folders(parent)` | ✅ | ✅ `listFunctionFolders()` | `builders/ddl.ts:435` |

### 2.10 Interner DDL

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 222 | `interner_dump()` (`.repo(r)` / `.since(e)`) | ✅ `ddl::interner_dump` | ❌ **Missing** | No TS builder, no TS wire type `InternerDumpOp`. See gap G7. |
| 223 | `interner_touch(names)` (`.repo(r)`) | ✅ `ddl::interner_touch` | ❌ **Missing** | No TS builder, no TS wire type `InternerTouchOp`. |

### 2.11 Access Control (ACL) — `admin.ts`

Wire source: `shamir-query-types/src/admin/access.rs`. Rust builder: `shamir-query-builder/src/ddl/access_control.rs` + `res.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 224 | `res::database(name)` | ✅ `ddl::res::database` | ✅ `refDatabase()` | `builders/admin.ts:50` |
| 225 | `res::store(db, store)` | ✅ | ✅ `refStore()` | `builders/admin.ts:54` |
| 226 | `res::table(db, store, table)` | ✅ | ✅ `refTable()` | `builders/admin.ts:58` |
| 227 | `res::function(name)` | ✅ | ✅ `refFunction()` | `builders/admin.ts:62` |
| 228 | `res::function_folder(segs)` | ❌ Rust `res.rs` has no folder helper | ✅ `refFunctionFolder()` | `builders/admin.ts:66` — **TS exceeds Rust**. |
| 229 | `res::function_namespace()` | ✅ | ✅ `refFunctionNamespace()` | `builders/admin.ts:70` |
| 230 | `chmod(resource, mode)` | ✅ | ✅ `chmod()` | `builders/admin.ts:104` |
| 231 | `chown(resource, owner)` | ✅ | ✅ `chown()` | `builders/admin.ts:116` — TS accepts `string|bigint|number`, auto-hashes username. |
| 232 | `chgrp(resource, group)` | ✅ | ✅ `chgrp()` | `builders/admin.ts:121` |
| 233 | `create_group(name)` | ✅ | ✅ `createGroup()` | `builders/admin.ts:125` |
| 234 | `drop_group(ref)` | ✅ | ✅ `dropGroup()` | `builders/admin.ts:129` |
| 235 | `add_group_member(ref, user)` | ✅ | ✅ `addGroupMember()` | `builders/admin.ts:141` — auto-hashes username. |
| 236 | `remove_group_member(ref, user)` | ✅ | ✅ `removeGroupMember()` | `builders/admin.ts:154` — auto-hashes. |
| 237 | `access_tree()` (`.depth(d)` / `.db(d)`) | ✅ | ✅ `accessTree()` | `builders/admin.ts:159` |
| 238 | `GroupRef` by name | ✅ wire enum | ✅ `groupName()` | `builders/admin.ts:94` |
| 239 | `GroupRef` by id | ✅ wire enum | ✅ `groupId()` | `builders/admin.ts:98` |

### 2.12 RBAC (Auth) — `admin.ts`

Wire source: `shamir-query-types/src/auth/types.rs`. Rust builder: `shamir-query-builder/src/ddl/auth.rs`.

| # | Capability | Rust builder | TS builder | Note |
|---|-----------|-------------|-----------|------|
| 240 | `create_user(name, password)` | ✅ | ✅ `createUser()` | `builders/admin.ts:184` |
| 241 | `.roles(roles)` | ✅ `CreateUser::roles` | ✅ `{roles}` | `builders/admin.ts:187` |
| 242 | `.profile(p)` | ✅ `CreateUser::profile` | ✅ `{profile}` | `builders/admin.ts:194` |
| 243 | `.database(d)` (scoped user) | ✅ `CreateUser::database` | ✅ `{database}` | `builders/admin.ts:195` |
| 244 | `drop_user(name)` (HMAC) | ✅ | ✅ `dropUser()` | `builders/admin.ts:200` |
| 245 | `create_role(name, permissions)` | ✅ | ✅ `createRole()` | `builders/admin.ts:211` |
| 246 | `drop_role(name)` (HMAC) | ✅ | ✅ `dropRole()` | `builders/admin.ts:219` |
| 247 | `grant_role(role, user)` | ✅ | ✅ `grantRole()` | `builders/admin.ts:230` |
| 248 | `revoke_role(role, user)` | ✅ | ✅ `revokeRole()` | `builders/admin.ts:234` |
| 249 | `Permission` (effect/actions/resource/where) | ✅ wire struct | ✅ `permission()` | `builders/admin.ts:168` |
| 250 | `Resource` scope constructors (global/db/repo/table) | ❌ Rust builder has no helpers | ✅ `scopeGlobal()` / `scopeDatabase()` / `scopeRepo()` / `scopeTable()` | `builders/admin.ts:76-90` — **TS exceeds Rust**. |

---

## 3. Prioritized Gap List (TS ↔ Rust parity)

### P0 — Functional gaps (TS cannot express something Rust can)

| Gap | Impact | TS file | Rust file | Fix |
|-----|--------|---------|-----------|-----|
| **G7** | ❌ **Interner DDL missing** — `interner_dump` and `interner_touch` have Rust builders (`ddl/interner.rs`) and wire types (`admin/types/interner_ops.rs`) but **zero TS coverage**: no type, no builder. | `types/ddl.ts` (absent) | `ddl/interner.rs` | Add `InternerDumpOp` / `InternerTouchOp` to `types/ddl.ts`; add `internerDump()` / `internerTouch()` to `builders/ddl.ts`. |
| **G5** | ❌ **No `Doc` builder** — Rust `write::Doc` lets users embed `$ref`/`$fn`/`$query` expressions in write values (insert/update/upsert). TS users can only pass plain JS objects — **computed write values are inaccessible** without hand-assembling wire shapes. | `builders/write.ts` (absent) | `write/doc.rs` | Add a `Doc` class or allow `FilterValue` in `WireValue` positions. |
| **G6** | ❌ **Subscribe `deliver_call` missing** — `DeliverMode::Call(CallOp)` is in the wire type (`types/subscribe.ts:44`) but the `subscribe()` builder has no path to produce it. | `builders/subscribe.ts:67` | `batch/subscribe.rs:110` | Add `source.call: CallOp` or a `deliverCall` option. |

### P1 — Ergonomic gaps (TS can express it, but less ergonomically)

| Gap | Impact | TS file | Rust file | Fix |
|-----|--------|---------|-----------|-----|
| **G1** | 🟡 **No inline `whereEq`/`whereGt`/... methods** — Rust's `where_methods!` macro generates 24+ inline filter-and-combine methods on `Query`. TS users must build a `Filter` object then call `.where()`. | `builders/query.ts` | `query/conds.rs` | Optional: add `whereEq(field, val)` etc. to `Query`. Low priority — the free `filter.*` functions are idiomatic TS. |
| **G3** | 🟡 **No typed `Handle`** — Rust's `Handle`/`RowRef` types generate correct `$query` paths (`[].field`, `[0].id`). TS users must manually construct path strings like `queryRef('@users', '[0].id')`. Error-prone. | `builders/batch.ts` | `batch/handle.rs` | Add a `Handle` class returned by `Batch.add()` with `.column()`, `.row()`, `.first()`, `.all()`. |
| **G4** | ❌ **No `try_build()` validation** — Rust validates `$query` ref aliases and `after` deps at build time. TS `build()` is unchecked — invalid refs surface only server-side. | `builders/batch.ts:207` | `batch/batch.rs:642` | Add a `tryBuild()` that walks the built object for `$query` keys and checks alias existence. |

### P2 — Minor / cosmetic

| Gap | Impact | Note |
|-----|--------|------|
| **G2** | 🟡 No `FilterExt` trait — TS `Filter` is a plain union, no chainable `.and()`/`.or()`. Idiomatic TS uses free functions. **Not a real gap.** |
| **G8** | 🟡 `SelectItem::Expression` (`type:'expr'`) — wire type exists (`types/query.ts:64`) with `expr: unknown`. No TS or Rust builder for `SelectExpr`. Both sides are wire-only. **Not a TS-specific gap.** |
| **G9** | 🟡 `lit_u64` missing — TS `number` covers the safe integer range; the wire `FilterValue::Int` is `i64` anyway. **Negligible.** |
| **G10** | 🟡 `bin()` constructor missing — `Uint8Array` is in the `FilterValue` type but no helper. Users pass raw `Uint8Array`. **Trivial.** |

---

## 4. Summary

- **TS exceeds Rust** in: `currentOnly()` retention helper (#212), `refFunctionFolder()` (#228), Resource scope constructors (#250), `filter.expr()` / `filter.cond()` builders (#80, #81), and `filter.computed()` with inline `exprArgs` (#66).
- **TS matches Rust** on all core OQL (select/filter/order/pagination/temporal) and the vast majority of DDL.
- **3 functional gaps** (P0): interner DDL, Doc builder, subscribe deliver_call.
- **3 ergonomic gaps** (P1): inline where-methods, typed Handle, try_build validation.

**Overall assessment**: The TS builder covers **~95%** of the Rust builder's surface. The gaps are concentrated in (a) Stage 5 interner ops (newer, likely not yet needed by TS clients), (b) computed write-value expressions (`Doc` — a design choice that may be deliberate given JS's object literals), and (c) subscribe `deliver_call` (niche). The typed `Handle` and `try_build` gaps are the most impactful for developer experience.
