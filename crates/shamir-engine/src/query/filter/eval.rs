//! Compatibility re-export shim.
//!
//! `eval.rs` has been split into `filter_callback.rs` and `filter_node.rs`.
//! This file re-exports everything so that existing paths of the form
//! `crate::query::filter::eval::*` keep compiling without any changes to
//! callers outside `query/filter/`.

pub use super::compile::compile_filter;
pub use super::filter_callback::FilterCallback;
pub use super::filter_node::{CompareOp, FilterNode};
pub use super::index_range::predicate_to_index_range;
pub use super::resolve::{
    compare_values, filter_value_to_inner, filter_value_to_query, intern_field_path, resolve_field,
    resolve_field_ref, resolve_filter_query, resolve_filter_value,
};
