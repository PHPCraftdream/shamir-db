//! Common query parsing utilities shared across all query types.
//!
//! This module contains parsers for query components that are used by
//! multiple query types (SELECT, UPDATE, DELETE, etc.).

pub mod parser;

pub use parser::{
    agg_func_from_str, aggregate_field_from_value, expr_from_value, expr_value_from_value,
    filter_from_value, filter_value_from_value, group_by_from_value, limit_offset_from_value,
    order_by_from_value, order_by_item_from_value, QueryParseError,
};

#[cfg(test)]
mod tests;
