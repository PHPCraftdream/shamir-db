//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.
//!
//! Pure DTO shapes (Filter, FilterValue, Cond, FilterExpr, FnCall, FieldPath)
//! live in `shamir-query-types::filter`. Re-exported here so existing
//! `crate::query::filter::*` paths inside the engine keep resolving.
//!
//! Evaluation runtime (`compile_filter`, `eval_context`)
//! stays in this crate because it touches Interner state.

pub mod caches;
pub mod evaluation;

// Module-path aliases preserving the historical `filter::*` module paths.
pub use caches::{cond_cache, field_path_cache, query_ref_cache};
pub use evaluation::filter_node;
pub use evaluation::planning::{compile, index_range};
pub use evaluation::runtime::core::{eval, eval_bytes, eval_context, read_budget};
pub use evaluation::semantics::{fts, pattern_guard, resolve};
// Crate-internal module (kept at the historical `filter::numeric_cmp` path).
pub(crate) use evaluation::semantics::numeric_cmp;

// Re-export DTOs from the shared types crate.
pub use compile::{compile_filter, try_compile_filter};
pub(crate) use compile::{compile_filter_with_context, try_compile_filter_with_context};
pub use cond_cache::{cond_cache_get, prescan_cond_cache, CondCache};
pub use eval_context::FilterContext;
pub use field_path_cache::{prescan_field_path_cache, FieldPathCache};
pub use filter_node::{CompareOp, FilterNode};
pub use index_range::predicate_to_index_range;
pub use pattern_guard::{
    check_filter_patterns, check_filter_value_patterns, check_select_expr_patterns,
    MAX_FILTER_PATTERN_LENGTH,
};
pub use query_ref_cache::{prescan_query_ref_cache, QueryRefCache};
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
