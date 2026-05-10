//! SelectExpr — simple expressions for computed fields.

use serde::{Deserialize, Serialize};

use crate::query::filter::FieldPath;

/// Simple expressions (for future expansion)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SelectExpr {
    // Arithmetic
    Add { left: Box<SelectExpr>, right: Box<SelectExpr> },
    Sub { left: Box<SelectExpr>, right: Box<SelectExpr> },
    Mul { left: Box<SelectExpr>, right: Box<SelectExpr> },
    Div { left: Box<SelectExpr>, right: Box<SelectExpr> },

    // Field reference
    Field { path: FieldPath },

    // Literal value
    Literal { value: SelectExprValue },
}

/// Expression values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelectExprValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}
