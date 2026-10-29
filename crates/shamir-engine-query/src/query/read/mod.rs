//! Read query types (SELECT)
//!
//! DTOs (ReadQuery, OrderBy, Pagination, QueryResult, QueryStats,
//! Select, SelectExpr, GroupBy, AggFunc, AggregateField) live in
//! `shamir-query-types::read`. Execution (`exec`) and query parser
//! stay here because they touch Interner / runtime state.

pub mod aggregation;
pub mod exec;
pub mod projection;

// Module-path aliases preserving the historical `read::*` module paths.
pub use aggregation as aggregate;
pub use projection::{order, select_projection};
// Crate-internal module (kept at the historical
// `read::hashable_query_value` path).
pub(crate) use projection::hashable_query_value;

// Re-export DTOs from the shared types crate.
pub use aggregate::{apply_aggregate_all, apply_group_by};
pub use exec::{apply_distinct_qv, apply_pagination, apply_select_value, has_aggregates};
pub use order::apply_order_by_qv;
pub use shamir_query_types::read::{
    AggFunc, AggregateField, At, CorruptRecordRef, DdlOpKind, DdlOpState, DdlOpStatus, ExplainPlan,
    GroupBy, NullsOrder, OrderBy, OrderByItem, OrderDirection, Pagination, PaginationInfo,
    PlanType, QueryRecord, QueryResult, QueryStats, ReadQuery, Select, SelectExpr, SelectExprValue,
    SelectItem, Temporal,
};

#[cfg(test)]
mod tests;
