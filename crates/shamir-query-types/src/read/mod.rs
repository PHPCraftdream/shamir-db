//! Read-query DTO module — query shape, ordering, pagination, results.
//! Execution (`exec`) and query parsing (`parser`) live in shamir-engine.

pub mod filter;
pub mod receipts;

mod clause;
mod core;

pub use clause::agg;
pub use clause::ddl;
pub use clause::group_by;
pub use clause::limit;
pub use clause::order_by;
pub use clause::temporal;
pub use core::query_record;
pub use core::query_result;
pub use core::read_query;
pub use core::select;
pub use core::select_expr;

pub use agg::{AggFunc, AggregateField};
pub use ddl::{DdlOpKind, DdlOpState, DdlOpStatus};
pub use group_by::GroupBy;
pub use limit::{Pagination, PaginationInfo};
pub use order_by::{NullsOrder, OrderBy, OrderByItem, OrderDirection};
pub use query_record::QueryRecord;
pub use query_result::{CorruptRecordRef, ExplainPlan, PlanType, QueryResult, QueryStats};
pub use read_query::ReadQuery;
pub use receipts::{VisibilityDependencyCode, VisibilityDependencyPending};
pub use select::{Select, SelectItem};
pub use select_expr::{SelectExpr, SelectExprValue};
pub use temporal::{At, Temporal};

#[cfg(test)]
mod tests;
