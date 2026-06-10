//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.
//!
//! Pure DTO shapes (Filter, FilterValue, Cond, FilterExpr, FnCall, FieldPath)
//! live in `shamir-query-types::filter`. Re-exported here so existing
//! `crate::query::filter::*` paths inside the engine keep resolving.
//!
//! Evaluation runtime (`compile_filter`, `eval_context`, `FilterCallback`)
//! stays in this crate because it touches Interner state.

pub mod compile;
pub mod eval;
pub mod eval_context;
pub mod filter_callback;
pub mod filter_node;
pub mod fts;
pub mod index_range;
pub mod resolve;

// Re-export DTOs from the shared types crate.
pub use compile::compile_filter;
pub use eval_context::FilterContext;
pub use filter_callback::FilterCallback;
pub use filter_node::{CompareOp, FilterNode};
pub use index_range::predicate_to_index_range;
pub use resolve::{
    compare_values, filter_value_to_inner, intern_field_path, resolve_field, resolve_field_ref,
    resolve_filter_value,
};
pub use shamir_query_types::filter::{
    Cond, FieldPath, Filter, FilterExpr, FilterExprOp, FilterValue, FnCall,
};

#[cfg(test)]
mod tests;
