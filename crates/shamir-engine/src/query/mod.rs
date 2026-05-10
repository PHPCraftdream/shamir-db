pub mod admin;
pub mod auth;
pub mod batch;
pub mod common;
pub mod filter;
pub mod read;
pub mod write;

pub use batch::{
    execute_batch, BatchError, BatchLimits, BatchOp, BatchPlan, BatchPlanner, BatchRequest,
    BatchResponse, QueryEntry, QueryPath, QueryReference, TableResolver,
};
pub use common::QueryParseError;
pub use filter::{FieldPath, Filter, FilterExpr, FilterExprOp, FilterValue};
pub use read::{
    AggFunc, AggregateField, GroupBy, NullsOrder, OrderBy,
    OrderByItem, OrderDirection, Pagination, PaginationInfo, QueryResult, QueryStats, ReadQuery,
    Select, SelectExpr, SelectExprValue, SelectItem,
};
// `TableRef` lives in shamir-query-types now; re-exported here so
// `crate::query::TableRef` paths inside the engine keep resolving.
pub use shamir_query_types::TableRef;
pub use write::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect, WriteResult};
