בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Coverage Audit: Rust Query Builder vs Wire Protocol

**Date:** 2025-06-24
**Auditor:** automated research pass
**Source of truth:** `crates/shamir-query-types/src/` (wire DTOs) + `crates/shamir-query-types/src/batch/batch_op.rs` (BatchOp dispatch enum)
**Under audit:** `crates/shamir-query-builder/src/`

## Methodology

Every variant of the `BatchOp` enum (the single dispatch surface for all
operations that travel inside a `BatchRequest`) was enumerated, plus the
top-level `DbRequest` variants (Ping, Execute, CreateScramUser, TxBegin /
TxExecute / TxCommit / TxRollback) that sit one layer above `BatchRequest`.

For each wire capability, the builder crate was searched for a constructor
function, builder struct, or `Batch::` method that produces it. Coverage is
marked:

- ✅ — the builder constructs this op with a dedicated, ergonomic API.
- 🟡 — partial: the op is constructible but some fields/sub-features are missing
  or require raw DTO assembly.
- ❌ — no builder path exists; the caller must hand-construct the wire DTO.

---

## Table 1: OQL (Data Query Language) Coverage

| # | Wire Capability | Wire DTO (`query-types`) | Builder Path | Status | Note |
|---|----------------|--------------------------|--------------|--------|------|
| **READ** | | | | | |
| 1 | Read query (SELECT) | `ReadQuery` / `BatchOp::Read` | `query/query.rs` → `Query::from()` / `Query::with_repo()` | ✅ | Full fluent builder: select, where, group_by, having, order_by, limit, offset, page, count_total, temporal, with_version |
| 2 | Select projection: wildcard (`*`) | `SelectItem::All` | `select/select_item.rs` → `select::all()` | ✅ | |
| 3 | Select projection: field (with alias) | `SelectItem::Field` | `select::field()`, `select::field_as()` | ✅ | |
| 4 | Select projection: aggregate (Count/Sum/Avg/Min/Max) | `SelectItem::Aggregate` | `select::agg()`, `agg_distinct()`, `sum()`, `avg()`, `min()`, `max()`, `count()` | ✅ | |
| 5 | Select projection: COUNT(*) | `SelectItem::CountAll` | `select::count_all()` | ✅ | |
| 6 | Select projection: funclib aggregate (median, stddev, …) | `SelectItem::AggregateFn` | `select::agg_fn()`, `agg_fn_distinct()` | ✅ | |
| 7 | Select projection: scalar function call | `SelectItem::Function` | `select::func()` | ✅ | |
| 8 | Select projection: expression (computed fields) | `SelectItem::Expression` | no builder | ❌ | `SelectExpr` (Add/Sub/Mul/Div/Field/Literal) has no constructor in the builder; callers must hand-build. Wire type is marked "future" in the DTO. |
| 9 | SELECT DISTINCT | `Select::distinct` | `Query::distinct()` | ✅ | |
| **FILTER** | | | | | |
| 10 | Eq / Ne / Gt / Gte / Lt / Lte | `Filter::{Eq,Ne,Gt,Gte,Lt,Lte}` | `filter/leaf.rs` → `eq()`, `ne()`, `gt()`, `gte()`, `lt()`, `lte()` | ✅ | Plus `where_eq()`, `or_where_eq()`, etc. on `Query`/`Conds` |
| 11 | FieldEq shortcut | `Filter::FieldEq` | `filter::field_eq()` | ✅ | |
| 12 | Like / ILike / Regex | `Filter::{Like,ILike,Regex}` | `filter::like()`, `ilike()`, `regex()` | ✅ | |
| 13 | IsNull / IsNotNull | `Filter::{IsNull,IsNotNull}` | `filter::is_null()`, `is_not_null()` | ✅ | |
| 14 | In / NotIn | `Filter::{In,NotIn}` | `filter::in_()`, `not_in()` | ✅ | |
| 15 | Contains / ContainsAny / ContainsAll | `Filter::{Contains,ContainsAny,ContainsAll}` | `filter::contains()`, `contains_any()`, `contains_all()` | ✅ | |
| 16 | Between | `Filter::Between` | `filter::between()` | ✅ | |
| 17 | Exists / NotExists | `Filter::{Exists,NotExists}` | `filter::exists()`, `not_exists()` | ✅ | |
| 18 | And / Or / Not (logical combinators) | `Filter::{And,Or,Not}` | `filter/combinators.rs` → `and()`, `or()`, `not()`, `FilterExt` trait | ✅ | Smart flattening on `.and()` / `.or()` |
| 19 | FTS (full-text search) | `Filter::Fts` | `filter::fts()` + `Query::fts()` | ✅ | |
| 20 | Vector similarity (top-k NN) | `Filter::VectorSimilarity` | `filter::vector_similarity()` | ✅ | No `Query::where_vector_similarity` method, but free function exists and can be passed to `Query::where_()`. |
| 21 | Computed (functional index comparison) | `Filter::Computed` | `filter::computed()`, `computed_with_args()` | ✅ | |
| **FILTER VALUES** | | | | | |
| 22 | Literal values (Null, Bool, Int, Float, String, Binary, Array) | `FilterValue::{Null,Bool,Int,Float,String,Binary,Array}` | `val/filter_value.rs` → `lit()`, `lit_u64()`, `bin()`, `null()`, `From` impls | ✅ | |
| 23 | Field reference (`$ref`) | `FilterValue::FieldRef` | `val::col()` | ✅ | |
| 24 | Query reference (`$query`) | `FilterValue::QueryRef` | `val::qref()`, `qref_all()` | ✅ | Auto-normalizes `@` prefix |
| 25 | Function call (`$fn`) | `FilterValue::FnCall` | `val::func()` | ✅ | Uses `FnCall::complex` form only; no simple-form helper (`FnCall::simple`) |
| 26 | Expression (`$expr`) | `FilterValue::Expr` / `FilterExpr` | `val::expr(...)` + обёртки | ✅ | DONE (B3, `DONE.md`) — конструкторы для всех 18 операторов. |
| 27 | Conditional (`$cond`) | `FilterValue::Cond` / `Cond` | `val::cond(...)` | ✅ | DONE (B3, `DONE.md`). |
| 28 | Parameter reference (`$param`) | `FilterValue::Param` | `val::param()` | ✅ | |
| **WRITE** | | | | | |
| 29 | Insert | `InsertOp` / `BatchOp::Insert` | `write/insert.rs` → `insert()`, `Insert::into()`, `with_repo()` | ✅ | `records_idmsgpack` always `Vec::new()` — the id-keyed msgpack pass-through path is not exposed. |
| 30 | Insert: id-keyed msgpack pass-through | `InsertOp::records_idmsgpack` | no builder | ❌ | The `ByteBuf` vector for pre-interned records is hardcoded to empty. No way to populate it from the builder. |
| 31 | Update | `UpdateOp` / `BatchOp::Update` | `write/update.rs` → `update()`, `Update::table()` | ✅ | Supports `where_`, `set`, `returning`, `returning_fields` |
| 32 | Upsert (Set) | `SetOp` / `BatchOp::Set` | `write/upsert.rs` → `upsert()`, `Upsert::table()` | ✅ | |
| 33 | Delete | `DeleteOp` / `BatchOp::Delete` | `write/delete.rs` → `delete()`, `Delete::from_table()` | ✅ | Panics if `.where_()` not called — deliberate safety guard |
| 34 | Doc (record-value builder) | — | `write/doc.rs` → `doc()`, `Doc::set()`, `set_value()` | ✅ | FilterValue→QueryValue round-trip for expressions |
| **BATCH** | | | | | |
| 35 | Batch request assembly | `BatchRequest` | `batch/batch.rs` → `Batch::new()`, `named()` | ✅ | Full: name, id, transactional, isolation, durability, return_all, return_flagged, return_only, limits |
| 36 | Batch: interner epochs | `BatchRequest::interner_epochs` | `Batch::interner_epochs(...)` | ✅ | DONE (B1, `DONE.md`). |
| 37 | Batch: result encoding | `BatchRequest::result_encoding` | `Batch::result_encoding(...)` | ✅ | DONE (B1, `DONE.md`) — `ResultEncoding::Id` доступен из билдера. |
| 38 | Sub-batch (nested BatchRequest + bind) | `SubBatchOp` / `BatchOp::Batch` | `Batch::sub_batch()`, `sub_batch_no_bind()` | ✅ | |
| 39 | QueryEntry `after` ordering deps | `QueryEntry::after` | `Batch::after()` | ✅ | |
| 40 | QueryEntry `return_result` flag | `QueryEntry::return_result` | `Batch::query_silent()`, `op_silent()` | ✅ | |
| **CALL** | | | | | |
| 41 | Stored procedure call | `CallOp` / `BatchOp::Call` | `Batch::call()`, `call_in_repo()` | ✅ | |
| **SUBSCRIBE** | | | | | |
| 42 | Subscribe to change events | `SubscribeOp` / `BatchOp::Subscribe` | `batch/subscribe.rs` → `Subscribe`, `SourceBuilder` + `Batch::subscribe()` | ✅ | Multi-source, filter, event mask, deliver modes (Records/Keys/Batch/Call), initial snapshot, from_version |
| 43 | Unsubscribe | `UnsubscribeOp` / `BatchOp::Unsubscribe` | `Batch::unsubscribe()` | ✅ | |
| **WIRE / RESPONSE** | | | | | |
| 44 | Wire encoding (msgpack / QueryValue) | — | `wire/mod.rs` → `ToWire` blanket trait | ✅ | `.to_msgpack()`, `.to_query_value()` on any `Serialize` |
| 45 | Response extraction | `BatchResponse` | `response/batch_response_ext.rs` → `BatchResponseExt` | ✅ | `result()`, `rows()`, `rows_as()`, `row_as()`, `get()`, `get_rows()`, `get_as()`, `execution_plan()`, `transaction()`, `is_committed()`, `abort_reason()` |
| **TOP-LEVEL DbRequest** | | | | | |
| 46 | Ping (health check) | `DbRequest::Ping` | no builder | ❌ | Trivially constructible by hand, but no builder method. |
| 47 | Execute (batch against a DB) | `DbRequest::Execute` | `Batch::build()` produces the inner `BatchRequest`, but wrapping into `DbRequest::Execute { db, batch }` is not builder-wrapped | 🟡 | The builder produces `BatchRequest`, not `DbRequest`. The client SDK wraps it. |
| 48 | CreateScramUser (wire-level user) | `DbRequest::CreateScramUser` | no builder | ❌ | Distinct from `BatchOp::CreateUser` (DB-level). No builder path. |
| 49 | TxBegin (interactive transaction open) | `DbRequest::TxBegin` | no builder | ❌ | |
| 50 | TxExecute (interactive transaction step) | `DbRequest::TxExecute` | no builder | ❌ | |
| 51 | TxCommit | `DbRequest::TxCommit` | no builder | ❌ | |
| 52 | TxRollback | `DbRequest::TxRollback` | no builder | ❌ | |

**OQL summary:** 52 capabilities enumerated. ✅ DONE с момента отчёта: FilterExpr
(`$expr`), Cond (`$cond`), interner_epochs, result_encoding — см. `DONE.md`.
Остаются ❌: SelectExpr, `InsertOp.records_idmsgpack` (B4), DbRequest-level ops.

---

## Table 2: DDL (Admin / Management Language) Coverage

| # | Wire Capability | Wire DTO (`query-types`) | Builder Path | Status | Note |
|---|----------------|--------------------------|--------------|--------|------|
| **DATABASE** | | | | | |
| 1 | Create database | `CreateDbOp` / `BatchOp::CreateDb` | `ddl/create_db.rs` → `create_db()` | ✅ | `if_not_exists` supported |
| 2 | Drop database | `DropDbOp` / `BatchOp::DropDb` | `ddl/drop_db.rs` → `drop_db()` | ✅ | `hmac`, `cascade` supported |
| **REPOSITORY** | | | | | |
| 3 | Create repository | `CreateRepoOp` / `BatchOp::CreateRepo` | `ddl/create_repo.rs` → `create_repo()` | ✅ | `engine`, `path`, `tables`, `if_not_exists` |
| 4 | Drop repository | `DropRepoOp` / `BatchOp::DropRepo` | `ddl/drop_repo.rs` → `drop_repo()` | ✅ | `hmac`, `cascade` |
| **TABLE** | | | | | |
| 5 | Create table | `CreateTableOp` / `BatchOp::CreateTable` | `ddl/create_table.rs` → `create_table()` | ✅ | `repo`, `if_not_exists`, `retention`, `schema` |
| 6 | Drop table | `DropTableOp` / `BatchOp::DropTable` | `ddl/drop_table.rs` → `drop_table()` | ✅ | `repo`, `hmac` |
| **INDEX** | | | | | |
| 7 | Create index (hash / unique / sorted / FTS / vector / functional / covering) | `CreateIndexOp` / `BatchOp::CreateIndex` | `ddl/create_index.rs` → `create_index()` | ✅ | Full builder: `fields`, `unique`, `sorted`, `index_type`, `fts_tokenizer`, `fts_language`, `functional_op`, `functional_args`, `vector_dim`, `vector_metric`, `include`, `if_not_exists` |
| 8 | Drop index | `DropIndexOp` / `BatchOp::DropIndex` | `ddl/drop_index.rs` → `drop_index()` | ✅ | `unique`, `repo`, `hmac` |
| **BUFFER CONFIG** | | | | | |
| 9 | Set buffer config | `SetBufferConfigOp` / `BatchOp::SetBufferConfig` | `ddl/buffer_config.rs` → `set_buffer_config()` | ✅ | Takes `BufferConfigDto` (re-exported as `BufConfig`) |
| 10 | Get buffer config | `GetBufferConfigOp` / `BatchOp::GetBufferConfig` | `ddl::get_buffer_config()` | ✅ | |
| 11 | Alter buffer config (partial patch) | `AlterBufferConfigOp` / `BatchOp::AlterBufferConfig` | `ddl::alter_buffer_config()` | ✅ | Takes `BufferConfigPatch` (re-exported as `BufPatch`) |
| **FUNCTION DDL** | | | | | |
| 12 | Create function (source or WASM) | `CreateFunctionOp` / `BatchOp::CreateFunction` | `ddl/function.rs` → `create_function()` | ✅ | `source`, `wasm`, `replace` |
| 13 | Drop function | `DropFunctionOp` / `BatchOp::DropFunction` | `ddl::drop_function()` | ✅ | |
| 14 | Rename function | `RenameFunctionOp` / `BatchOp::RenameFunction` | `ddl::rename_function()` | ✅ | |
| 15 | Create function folder | `CreateFunctionFolderOp` / `BatchOp::CreateFunctionFolder` | `ddl::create_function_folder()` | ✅ | |
| **VALIDATOR DDL** | | | | | |
| 16 | Create validator | `CreateValidatorOp` / `BatchOp::CreateValidator` | `ddl/validator.rs` → `create_validator()` | ✅ | `source`, `wasm`, `replace` |
| 17 | Drop validator | `DropValidatorOp` / `BatchOp::DropValidator` | `ddl::drop_validator()` | ✅ | |
| 18 | Rename validator | `RenameValidatorOp` / `BatchOp::RenameValidator` | `ddl::rename_validator()` | ✅ | |
| 19 | Bind validator to table | `BindValidatorOp` / `BatchOp::BindValidator` | `ddl::bind_validator()` | ✅ | `db`, `repo`, `ops`, `priority` |
| 20 | Unbind validator | `UnbindValidatorOp` / `BatchOp::UnbindValidator` | `ddl::unbind_validator()` | ✅ | |
| 21 | List validators for table | `ListValidatorsOp` / `BatchOp::ListValidators` | `ddl::list_validators()` | ✅ | |
| **SCHEMA DDL** | | | | | |
| 22 | Set table schema (whole-replace) | `SetTableSchemaOp` / `BatchOp::SetTableSchema` | `ddl/schema.rs` → `set_table_schema()` | ✅ | `repo`, `rules`, `expected_version` |
| 23 | Add schema rule | `AddSchemaRuleOp` / `BatchOp::AddSchemaRule` | `ddl::add_schema_rule()` | ✅ | |
| 24 | Remove schema rule | `RemoveSchemaRuleOp` / `BatchOp::RemoveSchemaRule` | `ddl::remove_schema_rule()` | ✅ | |
| 25 | Get table schema | `GetTableSchemaOp` / `BatchOp::GetTableSchema` | `ddl::get_table_schema()` | ✅ | |
| 26 | Field rule builder (constraints) | `FieldRuleDto` / `ConstraintsDto` | `ddl/schema.rs` → `field()` → `FieldBuilder` | 🟡 | Covers: string/int/f64/dec/bool/bin/list/map/any, required, nullable, unsigned, min/max/min_f64/max_f64, len/max_len/min_len, array_of, scalar, format, compare, unique, foreign_key. **Missing:** `one_of` constraint (the enum constraint `ConstraintsDto::one_of`), `set` and `null` type tags (no `.set()` / `.null_type()` methods — though `type_tag("set")` works as escape hatch). |
| **AUTH** | | | | | |
| 27 | Create user (DB-level) | `CreateUserOp` / `BatchOp::CreateUser` | `ddl/auth.rs` → `create_user()` | ✅ | `roles`, `profile`, `database` |
| 28 | Drop user | `DropUserOp` / `BatchOp::DropUser` | `ddl::drop_user()` | ✅ | `hmac` |
| 29 | Create role | `CreateRoleOp` / `BatchOp::CreateRole` | `ddl::create_role()` | ✅ | Takes `Vec<Permission>` |
| 30 | Drop role | `DropRoleOp` / `BatchOp::DropRole` | `ddl::drop_role()` | ✅ | `hmac` |
| 31 | Grant role | `GrantRoleOp` / `BatchOp::GrantRole` | `ddl::grant_role()` | ✅ | |
| 32 | Revoke role | `RevokeRoleOp` / `BatchOp::RevokeRole` | `ddl::revoke_role()` | ✅ | |
| **ACCESS CONTROL** | | | | | |
| 33 | chmod | `ChmodOp` / `BatchOp::Chmod` | `ddl/access_control.rs` → `chmod()` | ✅ | Takes `ResourceRef` |
| 34 | chown | `ChownOp` / `BatchOp::Chown` | `ddl::chown()` | ✅ | |
| 35 | chgrp | `ChgrpOp` / `BatchOp::Chgrp` | `ddl::chgrp()` | ✅ | `Option<u64>` for null-clear |
| 36 | Create group | `CreateGroupOp` / `BatchOp::CreateGroup` | `ddl::create_group()` | ✅ | |
| 37 | Drop group | `DropGroupOp` / `BatchOp::DropGroup` | `ddl::drop_group()` | ✅ | Takes `GroupRef` |
| 38 | Add group member | `AddGroupMemberOp` / `BatchOp::AddGroupMember` | `ddl::add_group_member()` | ✅ | |
| 39 | Remove group member | `RemoveGroupMemberOp` / `BatchOp::RemoveGroupMember` | `ddl::remove_group_member()` | ✅ | |
| 40 | Access tree introspection | `AccessTreeOp` / `BatchOp::AccessTree` | `ddl::access_tree()` | ✅ | `depth`, `db` |
| 41 | ResourceRef helpers | `ResourceRef` | `ddl/res.rs` → `database()`, `store()`, `table()`, `function()`, `function_namespace()` | 🟡 | **Missing:** `function_folder()` helper for `ResourceRef::FunctionFolder`. Caller must construct the enum variant directly. |
| **MIGRATION** | | | | | |
| 42 | Start migration | `StartMigrationOp` / `BatchOp::StartMigration` | `ddl/migration.rs` → `start_migration()` | ✅ | `repo`, `dst_path`, `hmac` |
| 43 | Commit migration | `CommitMigrationOp` / `BatchOp::CommitMigration` | `ddl::commit_migration()` | ✅ | `hmac` |
| 44 | Rollback migration | `RollbackMigrationOp` / `BatchOp::RollbackMigration` | `ddl::rollback_migration()` | ✅ | `hmac` |
| 45 | Migration status | `MigrationStatusOp` / `BatchOp::MigrationStatus` | `ddl::migration_status()` | ✅ | |
| **RETENTION / TEMPORAL** | | | | | |
| 46 | Set retention | `SetRetentionOp` / `BatchOp::SetRetention` | `ddl/retention.rs` → `set_retention()` | ✅ | |
| 47 | Purge history | `PurgeHistoryOp` / `BatchOp::PurgeHistory` | `ddl::purge_history()` | ✅ | Takes `PurgeScope` |
| 48 | Changes since (journal read) | `ChangesSinceOp` / `BatchOp::ChangesSince` | `ddl::changes_since()` | ✅ | `repo`, `limit` |
| **INTERNER** | | | | | |
| 49 | Interner dump | `InternerDumpOp` / `BatchOp::InternerDump` | `ddl/interner.rs` → `interner_dump()` | ✅ | `repo`, `since` |
| 50 | Interner touch | `InternerTouchOp` / `BatchOp::InternerTouch` | `ddl::interner_touch()` | ✅ | |
| 51 | Field resolver (client-side) | — (builder utility) | `ddl/interner_resolve.rs` → `FieldResolver` trait, `resolve_field_path()` | ✅ | Bonus: pure client-side name→id resolution |
| **LIST** | | | | | |
| 52 | List databases | `ListOp::Databases` / `BatchOp::List` | `ddl/list.rs` → `list_databases()` | ✅ | |
| 53 | List repos | `ListOp::Repos` | `ddl::list_repos()` | ✅ | |
| 54 | List tables | `ListOp::Tables` | `ddl::list_tables()` | ✅ | `repo` |
| 55 | List indexes | `ListOp::Indexes` | `ddl::list_indexes()` | ✅ | `table`, `repo` |
| 56 | List users | `ListOp::Users` | `ddl::list_users()` | ✅ | |
| 57 | List roles | `ListOp::Roles` | `ddl::list_roles()` | ✅ | |
| 58 | List functions | `ListOp::Functions` | `ddl::list_functions()` | ✅ | `folder` filter |
| 59 | List validators (catalogue-wide) | `ListOp::Validators` | `ddl::list_all_validators()` | ✅ | |
| 60 | List function folders | `ListOp::FunctionFolders` | `ddl::list_function_folders()` | ✅ | `parent` filter |

**DDL summary:** 60 capabilities enumerated. 0 ❌ missing. 2 🟡 partial (FieldBuilder missing `one_of` + `set`/`null` types, ResourceRef missing `function_folder` helper). The remaining 58 are ✅.

---

## Awkward / Incomplete Builder Surface

These are builder paths that *exist* but have friction:

1. **`FilterValue::FnCall` simple form** — `val::func()` always uses
   `FnCall::Complex { name, args }`. There is no constructor for the no-arg
   `FnCall::Simple(name)` wire variant (e.g. `{"$fn":"NOW"}`). Callers must
   construct `FilterValue::FnCall { call: FnCall::simple("NOW") }` manually.
   The `FnCall` type is re-exported from `query-types` but not from the
   builder's `val` module.

2. ~~**`Batch::build()` drops `interner_epochs` / `result_encoding`**~~ —
   ✅ DONE (B1, `DONE.md`): добавлены `Batch::interner_epochs(...)` и
   `Batch::result_encoding(...)`; `ResultEncoding::Id` доступен из билдера.

3. **`InsertOp.records_idmsgpack`** — the builder always produces an empty
   `Vec::new()`. The id-keyed msgpack pass-through path (the v2 wire
   optimization) has no builder entry point. Callers must hand-construct
   `InsertOp` with `records_idmsgpack` populated.

4. **`FieldBuilder` constraint gaps** — `one_of` (enum constraint), `set` type
   tag, and `null` type tag lack dedicated setters. `type_tag("set")` / 
   `type_tag("null")` work as escape hatches but `one_of` requires constructing
   `ConstraintsDto` manually.

5. **`ResourceRef::FunctionFolder`** — `ddl/res.rs` provides helpers for
   `Database`, `Store`, `Table`, `Function`, and `FunctionNamespace` but omits
   `FunctionFolder`.

6. **`Query` missing `where_vector_similarity`** — the `where_methods!` macro
   generates `where_*` methods for most filter types but `VectorSimilarity` and
   `Computed` are only available as free functions (`filter::vector_similarity`,
   `filter::computed`) which must be passed to `Query::where_()`.

7. **`DbRequest` envelope not wrapped** — the builder produces `BatchRequest`,
   not `DbRequest::Execute`. The `DbRequest` variants (Ping,
   CreateScramUser, TxBegin/TxExecute/TxCommit/TxRollback) have no builder
   coverage at all. This is by design for a WASM-compiled client crate (the
   transport layer wraps the envelope), but it means the builder does not cover
   the full wire surface.

---

## Prioritized Gap List

Ranked by impact (user-facing frequency × severity):

### High Priority

> ✅ DONE (см. `DONE.md`): `val::expr` (`$expr`, #26), `val::cond` (`$cond`,
> #27), `Batch::result_encoding()` (#37), `Batch::interner_epochs()` (#36).
> Прежние High-priority дыры билдера закрыты.

### Medium Priority

4. **`InsertOp.records_idmsgpack` builder (OQL #30, B4)** — Exposing the
   id-keyed msgpack path would let builder users opt into the v2 wire
   optimization. May require a `Doc::build_idmsgpack()` or an `Insert::row_idmsgpack(bytes)`
   method. **(остаётся)**

6. **`FieldBuilder::one_of()` (DDL #26)** — The enum constraint is a common
   schema-validation pattern. One-liner: `.one_of(vec![...])`.

7. **Interactive transaction `DbRequest` builders (OQL #49-52)** — TxBegin /
   TxExecute / TxCommit / TxRollback have no builder. These are top-level
   wire ops, not `BatchOp` variants. If the builder is intended as the sole
   client API, these need coverage.

### Low Priority

8. **`FnCall::Simple` helper (OQL #25)** — `val::func_simple("NOW")` for the
   no-arg shortcut form.

9. **`ResourceRef::FunctionFolder` helper (DDL #41)** — `res::function_folder(segments)`
   one-liner.

10. **`SelectItem::Expression` / `SelectExpr` (OQL #8)** — Marked "future" in
    the DTO; low urgency unless the engine begins executing these.

11. **`DbRequest::Ping` / `CreateScramUser` builders (OQL #46, #48)** — Trivial
    wire ops that are easy to hand-construct. Low frequency.

12. **`FieldBuilder::set()` / `.null_type()` (DDL #26)** — `type_tag("set")` /
    `type_tag("null")` escape hatches exist.

---

## Summary

| Domain | Capabilities Enumerated | ✅ Covered | 🟡 Partial | ❌ Missing |
|--------|------------------------|------------|------------|------------|
| **OQL** | 52 | 41 | 4 | 7 |
| **DDL** | 60 | 56 | 2 | 0 |
| **Total** | **112** | **97** | **6** | **7** |

The DDL surface is near-perfectly covered — every `BatchOp` admin variant has a
dedicated builder. The OQL gaps cluster around expression/value sub-languages
(`FilterExpr`, `Cond`, `SelectExpr`) that are richer than the leaf-constructor
pattern the builder currently favors, plus the `DbRequest`-level transaction
ops that sit above the batch layer.
