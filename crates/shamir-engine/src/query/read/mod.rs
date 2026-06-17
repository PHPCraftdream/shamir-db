//! Read query types (SELECT)
//!
//! DTOs (ReadQuery, OrderBy, Pagination, QueryResult, QueryStats,
//! Select, SelectExpr, GroupBy, AggFunc, AggregateField) live in
//! `shamir-query-types::read`. Execution (`exec`) and JSON parser
//! stay here because they touch Interner / runtime state.

pub mod aggregate;
pub mod exec;
pub(crate) mod hashable_json;
pub mod order;
mod parser;
pub mod select_projection;

// Re-export DTOs from the shared types crate.
pub use aggregate::{apply_aggregate_all, apply_group_by};
pub use exec::{
    apply_distinct, apply_distinct_qv, apply_pagination, apply_select, apply_select_to_bytes,
    apply_select_value, has_aggregates,
};
pub use order::{apply_order_by, apply_order_by_qv};
pub use parser::query_from_value;
pub use shamir_query_types::read::{
    AggFunc, AggregateField, At, GroupBy, NullsOrder, OrderBy, OrderByItem, OrderDirection,
    Pagination, PaginationInfo, QueryRecord, QueryResult, QueryStats, ReadQuery, Select,
    SelectExpr, SelectExprValue, SelectItem, Temporal,
};

#[cfg(test)]
mod tests;
