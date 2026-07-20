//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.
//!
//! Pure DTO shapes (Filter, FilterValue, Cond, FilterExpr, FnCall, FieldPath)
//! live in `shamir-query-types::filter`. Re-exported here so existing
//! `crate::query::filter::*` paths inside the engine keep resolving.
//!
//! Evaluation runtime (`compile_filter`, `eval_context`)
//! stays in this crate because it touches Interner state.

pub mod compile;
pub mod cond_cache;
pub mod eval;
pub mod eval_bytes;
pub mod eval_context;
pub mod field_path_cache;
pub mod filter_node;
pub mod fts;
pub mod index_range;
pub mod resolve;

// Re-export DTOs from the shared types crate.
pub use compile::compile_filter;
pub use cond_cache::{cond_cache_get, prescan_cond_cache, CondCache};
pub use eval_context::FilterContext;
pub use field_path_cache::{prescan_field_path_cache, FieldPathCache};
pub use filter_node::{CompareOp, FilterNode};
pub use index_range::predicate_to_index_range;
pub use resolve::{
    compare_values, filter_value_to_inner, filter_value_to_query, intern_field_path, resolve_field,
    resolve_field_ref, resolve_filter_query, resolve_filter_value,
};
// Crate-internal only (used by the ForEach executor to detect/resolve the
// "whole column" `@alias[].field` form of `over` — Epic04/B, #653).
pub(crate) use resolve::{is_column_query_ref, resolve_query_ref_column};
pub use shamir_query_types::filter::{
    filter_value_to_query_value, query_value_to_filter_value, Cond, FieldPath, Filter, FilterExpr,
    FilterExprOp, FilterValue, FnCall,
};

#[cfg(test)]
mod tests;
