//! Read query types (SELECT)
//!
//! DTOs (ReadQuery, OrderBy, Pagination, QueryResult, QueryStats,
//! Select, SelectExpr, GroupBy, AggFunc, AggregateField) live in
//! `shamir-query-types::read`. Execution (`exec`) and JSON parser
//! stay here because they touch Interner / runtime state.

pub mod exec;
pub(crate) mod hashable_json;
mod parser;
pub mod select_projection;

// Re-export DTOs from the shared types crate.
pub use parser::query_from_value;
pub use shamir_query_types::read::{
    AggFunc, AggregateField, At, GroupBy, NullsOrder, OrderBy, OrderByItem, OrderDirection,
    Pagination, PaginationInfo, QueryResult, QueryStats, ReadQuery, Select, SelectExpr,
    SelectExprValue, SelectItem, Temporal,
};

#[cfg(test)]
mod tests;
