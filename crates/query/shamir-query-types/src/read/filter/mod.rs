//! Filter DTO module — pure data shapes. Evaluation logic lives in
//! `shamir-engine::query::filter::{eval, eval_context}`.

pub mod cond;
pub mod filter_enum;
pub mod filter_expr;
pub mod filter_value;
pub mod fn_call;

#[cfg(test)]
mod tests;

pub use cond::Cond;
pub use filter_enum::{
    check_filter_depth, check_filter_depth_at, check_filter_value_depth, check_select_expr_depth,
    Filter, ValueCompareOp, MAX_FILTER_DEPTH,
};
pub use filter_expr::{FieldPath, FilterExpr, FilterExprOp};
pub use filter_value::{
    filter_value_to_query_value, query_value_to_filter_value, FilterValue,
    FilterValueConversionError,
};
pub use fn_call::FnCall;
