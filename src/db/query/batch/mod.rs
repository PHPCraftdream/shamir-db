//! Batch query execution module.
//!
//! This module provides the ability to execute multiple queries in a single request,
//! with automatic dependency detection and parallel execution optimization.
//!
//! # Features
//!
//! - **Named queries**: Each query has a unique alias for result referencing
//! - **Query references**: Use `$query` to reference other query results
//! - **Automatic dependency detection**: Builds a dependency graph from references
//! - **Parallel execution**: Independent queries run in parallel stages
//! - **Cycle detection**: Prevents circular dependencies
//! - **Security limits**: Configurable limits for query count, depth, and time
//!
//! # Quick Start
//!
//! ## JSON Request Format
//!
//! ```json
//! {
//!   "queries": [
//!     {
//!       "alias": "users",
//!       "query": { "from": "users" }
//!     },
//!     {
//!       "alias": "orders",
//!       "query": {
//!         "from": "orders",
//!         "where": {
//!           "op": "eq",
//!           "field": "user_id",
//!           "value": { "$query": "users[0].id" }
//!         }
//!       }
//!     }
//!   ]
//! }
//! ```
//!
//! ## Query Reference Syntax
//!
//! Reference other query results using the `$query` syntax:
//!
//! | Syntax | Description |
//! |--------|-------------|
//! | `{ "$query": "@users" }` | Entire result array |
//! | `{ "$query": "@users[0]" }` | First record |
//! | `{ "$query": "@users[]" }` | All records (for extraction) |
//! | `{ "$query": "@users[].id" }` | Column of IDs |
//! | `{ "$query": "@users[0].name" }` | Specific field |
//! | `{ "$query": "@users.count" }` | Result count |
//!
//! ## Execution Plan
//!
//! The planner creates stages for parallel execution:
//!
//! ```text
//! queries: [users, products, orders, stats]
//! dependencies:
//!   users -> {}
//!   products -> {}
//!   orders -> {users, products}
//!   stats -> {orders}
//!
//! stages: [[users, products], [orders], [stats]]
//! ```
//!
//! # Example: E-commerce Dashboard
//!
//! ```json
//! {
//!   "name": "dashboard",
//!   "queries": [
//!     {
//!       "alias": "user",
//!       "query": {
//!         "from": "users",
//!         "where": { "op": "eq", "field": "id", "value": 123 }
//!       }
//!     },
//!     {
//!       "alias": "orders",
//!       "query": {
//!         "from": "orders",
//!         "where": {
//!           "op": "eq",
//!           "field": "user_id",
//!           "value": { "$query": "user[0].id" }
//!         }
//!       }
//!     },
//!     {
//!       "alias": "order_items",
//!       "query": {
//!         "from": "order_items",
//!         "where": {
//!           "op": "in",
//!           "field": "order_id",
//!           "values": [{ "$query": "orders[].id" }]
//!         }
//!       }
//!     },
//!     {
//!       "alias": "products",
//!       "query": {
//!         "from": "products",
//!         "where": {
//!           "op": "in",
//!           "field": "id",
//!           "values": [{ "$query": "order_items[].product_id" }]
//!         }
//!       }
//!     }
//!   ],
//!   "return_all": true
//! }
//! ```

mod types;
mod reference;
mod planner;

pub use types::{
    BatchError, BatchLimits, BatchPlan, BatchRequest, BatchResponse, NamedQuery,
};
pub use reference::{QueryPath, QueryReference};
pub use planner::BatchPlanner;

#[cfg(test)]
mod tests;
