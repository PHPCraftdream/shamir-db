//! FilterExpr — expression ($expr) for arithmetic and string operations.

use serde::{Deserialize, Serialize};

use super::FilterValue;

/// Expression operator for $expr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterExprOp {
    // Math
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,

    // String
    Concat,
    Lower,
    Upper,
    Trim,
    Length,

    // Logic
    And,
    Or,
    Not,

    // Comparison (returns bool)
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// Expression ($expr) for arithmetic and string operations.
///
/// # Examples
///
/// ```text
/// { "$expr": { "op": "add", "args": [10, 20] } }
/// { "$expr": { "op": "mul", "args": [{ "$ref": "price" }, 1.1] } }
/// { "$expr": { "op": "concat", "args": [{ "$ref": "first" }, " ", { "$ref": "last" }] } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilterExpr {
    pub op: FilterExprOp,
    pub args: Vec<FilterValue>,
}

impl FilterExpr {
    /// Create a new expression
    pub fn new(op: FilterExprOp, args: Vec<FilterValue>) -> Self {
        FilterExpr { op, args }
    }

    /// Create an add expression
    pub fn add(args: Vec<FilterValue>) -> Self {
        FilterExpr::new(FilterExprOp::Add, args)
    }

    /// Create a mul expression
    pub fn mul(args: Vec<FilterValue>) -> Self {
        FilterExpr::new(FilterExprOp::Mul, args)
    }

    /// Create a concat expression
    pub fn concat(args: Vec<FilterValue>) -> Self {
        FilterExpr::new(FilterExprOp::Concat, args)
    }
}
