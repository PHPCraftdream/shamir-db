//! # ShamirDB Query Language
//!
//! Все запросы к ShamирDB идут через единый формат `BatchRequest` (MessagePack на проводе, `QueryValue` внутри).
//! Один запрос — это batch с одной операцией.
//!
//! ## BatchRequest
//!
//! ```text
//! {
//!   "id": "req-001",
//!   "queries": {
//!     "alias": { ... operation ... }
//!   }
//! }
//! ```text
//!
//! | Поле | Тип | Обязательно | Описание |
//! |------|-----|-------------|----------|
//! | `id` | any | да | ID запроса, возвращается в ответе для async correlation |
//! | `queries` | object | да | Map alias → операция |
//! | `name` | string | нет | Имя для логирования |
//! | `transactional` | bool | нет | MVCC транзакция (default: false) |
//! | `return_all` | bool | нет | Возвращать все результаты (default: true) |
//! | `return_only` | [string] | нет | Вернуть только эти alias'ы |
//! | `limits` | object | нет | Лимиты безопасности |
//!
//! ## BatchResponse
//!
//! ```text
//! {
//!   "id": "req-001",
//!   "results": {
//!     "alias": { "records": [...], "stats": {...}, "pagination": {...} }
//!   },
//!   "execution_plan": [["users", "products"], ["orders"]],
//!   "execution_time_us": 1234
//! }
//! ```text
//!
//! ## Операции
//!
//! Тип операции определяется по ключевому полю в `QueryValue`-объекте:
//!
//! | Ключ | Операция | Категория |
//! |------|----------|-----------|
//! | `from` | Read (SELECT) | DML |
//! | `insert_into` | Insert | DML |
//! | `update` | Update | DML |
//! | `delete_from` | Delete | DML |
//! | `set` (без `update`) | Set (upsert) | DML |
//! | `create_db` | Create database | DDL |
//! | `drop_db` | Drop database | DDL |
//! | `create_repo` | Create repository | DDL |
//! | `drop_repo` | Drop repository | DDL |
//! | `create_table` | Create table | DDL |
//! | `drop_table` | Drop table | DDL |
//! | `create_index` | Create index | DDL |
//! | `drop_index` | Drop index | DDL |
//! | `list` | List entities | DDL |
//! | `create_user` | Create user | Auth |
//! | `drop_user` | Drop user | Auth |
//! | `create_role` | Create role | Auth |
//! | `drop_role` | Drop role | Auth |
//! | `grant_role` | Grant role to user | Auth |
//! | `revoke_role` | Revoke role from user | Auth |
//!
//! ---
//!
//! ## Read (SELECT)
//!
//! ```text
//! {
//!   "from": "users",
//!   "select": { ... },
//!   "where": { ... },
//!   "group_by": { ... },
//!   "order_by": { ... },
//!   "limit": { ... },
//!   "count_total": true
//! }
//! ```text
//!
//! ### from — таблица
//!
//! ```text
//! "from": "users"                    // repo "main" по умолчанию
//! "from": ["archive", "logs"]       // явный repo
//! ```text
//!
//! ### select
//!
//! ```text
//! "select": {
//!   "items": [
//!     {"type": "all"},
//!     {"type": "field", "path": ["name"]},
//!     {"type": "field", "path": ["user", "address", "city"], "alias": "city"},
//!     {"type": "count_all", "alias": "total"},
//!     {"type": "aggregate", "func": "sum", "field": ["amount"], "alias": "total_amount"},
//!     {"type": "aggregate", "func": "avg", "field": ["score"]}
//!   ],
//!   "distinct": true
//! }
//! ```text
//!
//! Агрегатные функции: `count`, `sum`, `avg`, `min`, `max`.
//!
//! ### where — фильтры
//!
//! #### Сравнение
//!
//! ```text
//! {"op": "eq",  "field": ["status"], "value": "active"}
//! {"op": "ne",  "field": ["role"],   "value": "guest"}
//! {"op": "gt",  "field": ["age"],    "value": 18}
//! {"op": "gte", "field": ["score"],  "value": 90}
//! {"op": "lt",  "field": ["price"],  "value": 100}
//! {"op": "lte", "field": ["count"],  "value": 10}
//! ```text
//!
//! #### Логические
//!
//! ```text
//! {"op": "and", "filters": [ ... ]}
//! {"op": "or",  "filters": [ ... ]}
//! {"op": "not", "filter": { ... }}
//! ```text
//!
//! #### Null / Exists
//!
//! ```text
//! {"op": "is_null",     "field": ["email"]}
//! {"op": "is_not_null", "field": ["email"]}
//! {"op": "exists",      "field": ["metadata"]}
//! {"op": "not_exists",  "field": ["deleted_at"]}
//! ```text
//!
//! #### In / Not In
//!
//! ```text
//! {"op": "in",     "field": ["status"], "values": ["active", "pending"]}
//! {"op": "not_in", "field": ["role"],   "values": ["banned", "suspended"]}
//! ```text
//!
//! #### Паттерны
//!
//! ```text
//! {"op": "like",  "field": ["name"], "pattern": "%alice%"}
//! {"op": "ilike", "field": ["name"], "pattern": "%ALICE%"}
//! {"op": "regex", "field": ["email"], "pattern": "^[a-z]+@"}
//! ```text
//!
//! #### Содержание (строки, массивы, множества)
//!
//! ```text
//! {"op": "contains",     "field": ["tags"], "value": "urgent"}
//! {"op": "contains_any", "field": ["tags"], "values": ["urgent", "critical"]}
//! {"op": "contains_all", "field": ["tags"], "values": ["reviewed", "approved"]}
//! ```text
//!
//! #### Диапазон
//!
//! ```text
//! {"op": "between", "field": ["age"], "from": 18, "to": 65}
//! ```text
//!
//! #### Ссылка на другое поле
//!
//! ```text
//! {"op": "gt", "field": ["salary"], "value": {"$ref": ["min_salary"]}}
//! ```text
//!
//! #### Ссылка на результат другого запроса ($query)
//!
//! ```text
//! {"op": "eq", "field": ["user_id"], "value": {"$query": "users", "path": "[0].id"}}
//! {"op": "in", "field": ["status"],  "values": [{"$query": "allowed_statuses", "path": "[].code"}]}
//! ```text
//!
//! ### Field paths
//!
//! Пути к полям — массивы строк:
//!
//! ```text
//! ["name"]                     // простое поле
//! ["user", "address", "city"]  // вложенное поле
//! ```text
//!
//! ### group_by
//!
//! ```text
//! "group_by": {
//!   "fields": [["city"], ["status"]],
//!   "having": {"op": "gt", "field": ["count"], "value": 5}
//! }
//! ```text
//!
//! ### order_by
//!
//! ```text
//! "order_by": {
//!   "items": [
//!     {"field": ["age"], "direction": "desc"},
//!     {"field": ["name"], "direction": "asc", "nulls": "last"}
//!   ]
//! }
//! ```text
//!
//! Direction: `asc` (default), `desc`. Nulls: `first`, `last`.
//!
//! ### limit (pagination)
//!
//! ```text
//! "limit": {"mode": "LimitOffset", "limit": 10, "offset": 20}
//! "limit": {"mode": "Page", "page": 3, "page_size": 25}
//! ```text
//!
//! ---
//!
//! ## Insert
//!
//! ```text
//! {
//!   "insert_into": "users",
//!   "values": [
//!     {"name": "Alice", "email": "alice@example.com"},
//!     {"name": "Bob", "email": "bob@example.com"}
//!   ]
//! }
//! ```text
//!
//! Возвращает вставленные записи с `_id`.
//!
//! ## Update
//!
//! ```text
//! {
//!   "update": "users",
//!   "where": {"op": "eq", "field": ["status"], "value": "inactive"},
//!   "set": {"status": "active", "updated_at": 1234567890},
//!   "select": {"return_mode": "changed"}
//! }
//! ```text
//!
//! | return_mode | Возвращает |
//! |-------------|-----------|
//! | `changed` (default) | Только изменённые записи |
//! | `unchanged` | Только совпавшие но не изменённые |
//! | `all` | Все совпавшие |
//!
//! `where` — опционально. Без него — обновляет все записи.
//!
//! ## Delete
//!
//! ```text
//! {
//!   "delete_from": "users",
//!   "where": {"op": "eq", "field": ["status"], "value": "deleted"}
//! }
//! ```text
//!
//! `where` — обязательно (защита от случайного удаления всего).
//!
//! ## Set (upsert)
//!
//! ```text
//! {
//!   "set": "users",
//!   "key": {"email": "alice@example.com"},
//!   "value": {"email": "alice@example.com", "name": "Alice", "status": "active"}
//! }
//! ```text
//!
//! Ищет запись по `key`. Если найдена — merge `value` в существующую. Если нет — insert. Возвращает `_created: true/false`.
//!
//! ---
//!
//! ## Зависимости ($query)
//!
//! Запросы в batch могут ссылаться на результаты друг друга:
//!
//! ```text
//! {
//!   "id": 1,
//!   "queries": {
//!     "active_users": {
//!       "from": "users",
//!       "where": {"op": "eq", "field": ["status"], "value": "active"}
//!     },
//!     "their_orders": {
//!       "from": "orders",
//!       "where": {
//!         "op": "in",
//!         "field": ["user_id"],
//!         "values": [{"$query": "active_users", "path": "[].id"}]
//!       }
//!     }
//!   }
//! }
//! ```text
//!
//! Планировщик автоматически:
//! 1. Извлекает зависимости из `$query` ссылок
//! 2. Проверяет циклы
//! 3. Строит параллельные стейджи: `[["active_users"], ["their_orders"]]`
//!
//! Каждый запрос видит только результаты своих объявленных зависимостей.
//!
//! ## Индексы
//!
//! Запросы с `where: eq` или `where: in` на индексированных полях автоматически используют index scan вместо full table scan. `stats.index_used` в ответе показывает какой индекс был использован.
//!
//! ---
//!
//! ## DDL (Admin Operations)
//!
//! ### Create/Drop Database
//!
//! ```text
//! {"create_db": "mydb"}
//! {"drop_db": "mydb"}
//! ```text
//!
//! ### Create/Drop Repository
//!
//! ```text
//! {"create_repo": "hot_cache", "engine": "in_memory", "tables": ["sessions", "tokens"]}
//! {"drop_repo": "hot_cache"}
//! ```text
//!
//! Engines: `in_memory` (всегда), `fjall` (если соответствующая cargo-feature
//! включена при сборке). Disk-движкам требуется поле `path`.
//!
//! ### Create/Drop Table
//!
//! ```text
//! {"create_table": "products", "repo": "main"}
//! {"drop_table": "products", "repo": "main"}
//! ```text
//!
//! `repo` — default `"main"`.
//!
//! ### Create/Drop Index
//!
//! ```text
//! {"create_index": "email_idx", "table": "users", "fields": [["email"]], "unique": true}
//! {"drop_index": "email_idx", "table": "users"}
//! ```text
//!
//! | Поле | Тип | Обязательно | Описание |
//! |------|-----|-------------|----------|
//! | `create_index` / `drop_index` | string | да | Имя индекса |
//! | `table` | string | да | Таблица |
//! | `fields` | [[string]] | да (create) | Пути полей |
//! | `unique` | bool | нет | Уникальный индекс (default: false) |
//! | `repo` | string | нет | Репозиторий (default: "main") |
//!
//! ### List
//!
//! ```text
//! {"list": "databases"}
//! {"list": "repos"}
//! {"list": "tables", "repo": "main"}
//! {"list": "indexes", "table": "users", "repo": "main"}
//! ```text
//!
//! ---
//!
//! ## Auth (Roles & Permissions)
//!
//! Реализовано: пользователи, роли, permissions с row-level security через
//! `SessionPermissions`. Подробная документация: [auth/README.md](./auth/README.md).
//!
//! ### Users
//!
//! ```text
//! {"create_user": "alice", "password": "...", "roles": ["readonly"]}
//! {"drop_user": "alice"}
//! {"grant_role": "analyst", "user": "alice"}
//! {"revoke_role": "analyst", "user": "alice"}
//! {"list": "users"}
//! ```text
//!
//! ### Roles
//!
//! ```text
//! {
//!   "create_role": "analyst",
//!   "permissions": [
//!     {
//!       "effect": "allow",
//!       "actions": ["read"],
//!       "resource": {"scope": "global"}
//!     },
//!     {
//!       "effect": "allow",
//!       "actions": ["insert", "update"],
//!       "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "reports"},
//!       "where": {"op": "eq", "field": ["department"], "value": "analytics"}
//!     }
//!   ]
//! }
//! {"drop_role": "analyst"}
//! {"list": "roles"}
//! ```text
//!
//! ### Permission structure
//!
//! ```text
//! {
//!   "effect": "allow",
//!   "actions": ["read", "insert"],
//!   "resource": {"scope": "table", "database": "mydb", "repo": "main", "table": "orders"},
//!   "where": { ... filter ... }
//! }
//! ```text
//!
//! `where` — row-level security. Тот же синтаксис фильтров. Автоматически добавляется ко всем запросам пользователя.
//! `$user` — ссылка на поля текущего пользователя: `{"$user": ["office", "region"]}`.
//!
//! ---
//!
//! ## Лимиты безопасности
//!
//! ```text
//! "limits": {
//!   "max_queries": 50,
//!   "max_dependency_depth": 10,
//!   "max_execution_time_secs": 30,
//!   "max_result_size": 10485760
//! }
//! ```text

pub mod admin;
pub mod auth;
pub mod batch;
pub mod filter;
pub mod read;
pub mod write;

pub use batch::{
    execute_batch, BatchError, BatchLimits, BatchOp, BatchPlan, BatchPlanner, BatchRequest,
    BatchResponse, QueryEntry, QueryPath, QueryReference, TableResolver,
};
pub use filter::{FieldPath, Filter, FilterExpr, FilterExprOp, FilterValue};
pub use read::{
    AggFunc, AggregateField, DdlOpKind, DdlOpState, DdlOpStatus, GroupBy, NullsOrder, OrderBy,
    OrderByItem, OrderDirection, Pagination, PaginationInfo, QueryResult, QueryStats, ReadQuery,
    Select, SelectExpr, SelectExprValue, SelectItem,
};
// `TableRef` lives in shamir-query-types now; re-exported here so
// `crate::query::TableRef` paths inside the engine keep resolving.
pub use shamir_query_types::call::CallOp;
pub use shamir_query_types::TableRef;
pub use write::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect, WriteResult};
