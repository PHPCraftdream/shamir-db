//! Batch query execution module.
//!
//! **Batch — единая точка входа для всех запросов к S.H.A.M.I.R. Database.**
//!
//! Этот мод предоставляет унифицированный msgpack-интерфейс для выполнения запросов.
//! Используйте Batch API для всех операций — от простых запросов до сложных
//! многозапросных транзакций с зависимостями.
//!
//! # Почему Batch?
//!
//! | Подход | Пример |
//! |--------|--------|
//! | Один запрос | `{ "queries": { "q": { "from": "table" } } }` |
//! | Несколько запросов | Map с автоматическим параллелизмом |
//! | С зависимостями | `{ "$query": "@users", "path": "[0].id" }` |
//!
//! ## Преимущества
//!
//! - **Единый формат** — все запросы через QueryValue / msgpack
//! - **Автоматический параллелизм** — независимые запросы выполняются одновременно
//! - **Ссылки на результаты** — используй результаты одного запроса в другом
//! - **Валидация** — проверка зависимостей, циклов, лимитов
//! - **Транзакции** — опциональная MVCC изоляция
//!
//! # Quick Start
//!
//! ## Простой запрос (аналог обычного Query)
//!
//! ```text
//! {
//!   "queries": {
//!     "users": {
//!       "from": "users",
//!       "where": { "op": "eq", "field": "status", "value": "active" },
//!       "limit": 10
//!     }
//!   }
//! }
//! ```
//!
//! ## Запросы с зависимостями
//!
//! ```text
//! {
//!   "queries": {
//!     "user": { "from": "users", "where": { "op": "eq", "field": "id", "value": 123 } },
//!     "orders": {
//!       "from": "orders",
//!       "where": {
//!         "op": "eq",
//!         "field": "user_id",
//!         "value": { "$query": "@user", "path": "[0].id" }
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! # Query Reference Syntax
//!
//! Ссылка на результаты другого запроса в том же batch'е — пара
//! `{ "$query": "@<alias>", "path": "<path>" }`. Префикс `@` —
//! явный маркер reference (отличает от литеральной строки); сервер
//! стрипает его перед lookup в queries map.
//!
//! | Syntax                                              | Description |
//! |-----------------------------------------------------|-------------|
//! | `{ "$query": "@users" }`                            | Весь массив результатов |
//! | `{ "$query": "@users", "path": "[0]" }`             | Первая запись |
//! | `{ "$query": "@users", "path": "[0].name" }`        | Поле первой записи |
//! | `{ "$query": "@users", "path": "[].id" }`           | Колонка id из всех записей (для `in`) |
//!
//! # Execution Plan
//!
//! Планировщик автоматически создаёт стадии для параллельного выполнения:
//!
//! ```text
//! queries: { users, products, orders, stats }
//! dependencies:
//!   users    -> {}
//!   products -> {}
//!   orders   -> {users, products}
//!   stats    -> {orders}
//!
//! stages: [[users, products], [orders], [stats]]
//! ```
//!
//! Stage 1: `users` и `products` параллельно
//! Stage 2: `orders` после Stage 1
//! Stage 3: `stats` после Stage 2
//!
//! # Полная документация
//!
//! См. [README.md](./README.md) для:
//! - Полного описания формата запроса
//! - Всех операторов фильтрации
//! - Лимитов безопасности
//! - Примеров использования
//! - Архитектуры модуля
//!
//! # Example: E-commerce Dashboard
//!
//! ```text
//! {
//!   "name": "dashboard",
//!   "queries": {
//!     "user": {
//!       "from": "users",
//!       "where": { "op": "eq", "field": "id", "value": 123 }
//!     },
//!     "orders": {
//!       "from": "orders",
//!       "where": {
//!         "op": "eq",
//!         "field": "user_id",
//!         "value": { "$query": "@user", "path": "[0].id" }
//!       }
//!     },
//!     "items": {
//!       "from": "order_items",
//!       "where": {
//!         "op": "in",
//!         "field": "order_id",
//!         "values": [{ "$query": "@orders", "path": "[].id" }]
//!       }
//!     },
//!     "products": {
//!       "from": "products",
//!       "where": {
//!         "op": "in",
//!         "field": "id",
//!         "values": [{ "$query": "items[].product_id" }]
//!       }
//!     }
//!   },
//!   "return_all": true
//! }
//! ```

mod batch_execute;
mod batch_validate;
mod execution_deadline;
mod executor_traits;
mod fk_actions;
mod fk_on_update;
mod fk_restrict;
mod interactive_tx;
mod param_subst;
mod query_runner;

// Only the executor (which actually drives a TableManager) lives here.
// DTOs, the topological planner, and the `$query` reference parser are
// all pure-data and live in `shamir-query-types::batch` — re-exported
// here so callers keep using `shamir_db::query::batch::*` paths.
pub use execution_deadline::ExecutionDeadline;
pub use executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
pub use query_runner::{
    commit_interactive_tx, execute_batch, execute_in_open_tx, open_interactive_tx, QueryRunner,
};
pub use shamir_query_types::batch::{
    BatchError, BatchLimits, BatchOp, BatchPlan, BatchPlanner, BatchRequest, BatchResponse,
    InternerDelta, QueryEntry, QueryPath, QueryReference, ReferenceParseError, ResultEncoding,
    TransactionInfo,
};

// Retained for engine-level unit tests; not accessible outside test builds.
#[cfg(test)]
pub(crate) use query_runner::execute_batch_with_permissions;

#[cfg(test)]
mod tests;
