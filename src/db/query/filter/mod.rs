//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.

mod cond;
mod filter_enum;
mod filter_expr;
mod filter_value;
mod fn_call;

/// Field path (e.g., "user.email" or "tags")
pub type FieldPath = String;

pub use cond::Cond;
pub use filter_enum::Filter;
pub use filter_expr::{FilterExpr, FilterExprOp};
pub use filter_value::FilterValue;
pub use fn_call::FnCall;

#[cfg(test)]
mod tests;
