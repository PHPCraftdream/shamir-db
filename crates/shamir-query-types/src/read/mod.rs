//! Read-query DTO module — query shape, ordering, pagination, results.
//! Execution (`exec`) and JSON parsing (`parser`) live in shamir-engine.

pub mod agg;
pub mod group_by;
pub mod limit;
pub mod order_by;
pub mod query_result;
pub mod read_query;
pub mod select;
pub mod select_expr;
pub mod temporal;

pub use agg::{AggFunc, AggregateField};
pub use group_by::GroupBy;
pub use limit::{Pagination, PaginationInfo};
pub use order_by::{NullsOrder, OrderBy, OrderByItem, OrderDirection};
pub use query_result::{QueryResult, QueryStats};
pub use read_query::ReadQuery;
pub use select::{Select, SelectItem};
pub use select_expr::{SelectExpr, SelectExprValue};
pub use temporal::{At, Temporal};

#[cfg(test)]
mod tests;
