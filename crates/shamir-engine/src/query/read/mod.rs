//! Read query types (SELECT)
//!
//! DTOs (ReadQuery, OrderBy, Pagination, QueryResult, QueryStats,
//! Select, SelectExpr, GroupBy, AggFunc, AggregateField) live in
//! `shamir-query-types::read`. Execution (`exec`) and JSON parser
//! stay here because they touch Interner / runtime state.

pub mod exec;
mod parser;

// Re-export DTOs from the shared types crate.
pub use parser::query_from_value;
pub use shamir_query_types::read::{
    AggFunc, AggregateField, GroupBy, NullsOrder, OrderBy, OrderByItem, OrderDirection, Pagination,
    PaginationInfo, QueryResult, QueryStats, ReadQuery, Select, SelectExpr, SelectExprValue,
    SelectItem,
};

#[cfg(test)]
mod tests;
