//! Compatibility re-export shim.
//!
//! `eval.rs` has been split into `filter_callback.rs` and `filter_node.rs`.
//! This file re-exports everything so that existing paths of the form
//! `crate::query::filter::eval::*` keep compiling without any changes to
//! callers outside `query/filter/`.

pub use super::filter_callback::FilterCallback;
pub use super::filter_node::{
    compare_values, compile_filter, filter_value_to_inner, intern_field_path,
    predicate_to_index_range, resolve_field, resolve_field_ref, resolve_filter_value, CompareOp,
    FilterNode,
};
