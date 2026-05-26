//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.
//!
//! Pure DTO shapes (Filter, FilterValue, Cond, FilterExpr, FnCall, FieldPath)
//! live in `shamir-query-types::filter`. Re-exported here so existing
//! `crate::query::filter::*` paths inside the engine keep resolving.
//!
//! Evaluation runtime (`compile_filter`, `eval_context`, `FilterCallback`)
//! stays in this crate because it touches Interner state.

pub mod eval;
pub mod eval_context;

// Re-export DTOs from the shared types crate.
pub use eval::{
    compare_values, compile_filter, filter_value_to_inner, intern_field_path, resolve_field,
    FilterCallback,
};
pub use eval_context::FilterContext;
pub use shamir_query_types::filter::{
    Cond, FieldPath, Filter, FilterExpr, FilterExprOp, FilterValue, FnCall,
};

#[cfg(test)]
mod tests;
