//! Filter DTO module — pure data shapes. Evaluation logic lives in
//! `shamir-engine::query::filter::{eval, eval_context}`.

pub mod cond;
pub mod filter_enum;
pub mod filter_expr;
pub mod filter_value;
pub mod fn_call;

pub use cond::Cond;
pub use filter_enum::{check_filter_depth, Filter, MAX_FILTER_DEPTH};
pub use filter_expr::{FilterExpr, FilterExprOp};
pub use filter_value::FilterValue;
pub use fn_call::FnCall;

/// Path into a record's field tree. `["address", "city"]` for nested
/// fields; `["name"]` for a top-level field.
pub type FieldPath = Vec<String>;
