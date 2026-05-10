//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.

mod cond;
pub mod eval;
pub mod eval_context;
mod filter_enum;
mod filter_expr;
mod filter_value;
mod fn_call;

/// Field path as a sequence of key segments.
///
/// Example: `["user", "address", "city"]` instead of `"user.address.city"`.
/// Simple field: `["name"]`.
pub type FieldPath = Vec<String>;

pub use cond::Cond;
pub use filter_enum::Filter;
pub use filter_expr::{FilterExpr, FilterExprOp};
pub use filter_value::FilterValue;
pub use eval::{compile_filter, compare_values, filter_value_to_inner, intern_field_path, resolve_field, FilterCallback};
pub use eval_context::FilterContext;
pub use fn_call::FnCall;

#[cfg(test)]
mod tests;
