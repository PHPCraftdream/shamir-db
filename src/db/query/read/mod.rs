//! Read query types (SELECT)
//!
//! Types for building SELECT queries.

mod group;
mod parser;
mod query;
mod select;

pub use group::{GroupBy, LimitOffset, NullsOrder, OrderBy, OrderByItem, OrderDirection};
pub use parser::query_from_value;
pub use query::{Query, QueryResult, QueryStats, TableName};
pub use select::{AggFunc, AggregateField, Expr, ExprValue, Select, SelectItem};

/// Alias for Query - makes API clearer when distinguishing read vs write operations.
pub type ReadQuery = Query;

#[cfg(test)]
mod tests;
