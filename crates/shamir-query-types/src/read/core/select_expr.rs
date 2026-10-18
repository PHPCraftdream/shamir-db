//! SelectExpr — simple expressions for computed fields.

use serde::{Deserialize, Serialize};

use crate::filter::{FieldPath, FilterExpr, FilterExprOp, FilterValue};

/// Simple expressions (for future expansion)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SelectExpr {
    // Arithmetic
    Add {
        left: Box<SelectExpr>,
        right: Box<SelectExpr>,
    },
    Sub {
        left: Box<SelectExpr>,
        right: Box<SelectExpr>,
    },
    Mul {
        left: Box<SelectExpr>,
        right: Box<SelectExpr>,
    },
    Div {
        left: Box<SelectExpr>,
        right: Box<SelectExpr>,
    },

    // Field reference
    Field {
        path: FieldPath,
    },

    // Literal value
    Literal {
        value: SelectExprValue,
    },
}

impl SelectExpr {
    /// Translate this `SelectExpr` tree into the equivalent [`FilterValue`]
    /// shape, so `SelectItem::Expression` can be evaluated through the SAME
    /// `resolve_filter_query` pipeline `SelectItem::Function`'s `args`
    /// already use (`SelectProjection::new`'s `funcs` vec) — see #1024.
    ///
    /// `SelectExpr`'s 6 variants are a strict, narrower duplicate of
    /// `FilterExpr`/`FilterValue`'s already-implemented-and-evaluated shape:
    /// `Add`/`Sub`/`Mul`/`Div` map onto the identical `FilterExprOp`
    /// variants, `Field { path }` maps onto `FilterValue::FieldRef { path }`
    /// (both share the exact same `FieldPath = Vec<String>` type — no lossy
    /// conversion needed), and each `SelectExprValue` literal maps 1:1 onto
    /// its `FilterValue` literal counterpart.
    pub fn to_filter_value(&self) -> FilterValue {
        match self {
            SelectExpr::Add { left, right } => binary_op(FilterExprOp::Add, left, right),
            SelectExpr::Sub { left, right } => binary_op(FilterExprOp::Sub, left, right),
            SelectExpr::Mul { left, right } => binary_op(FilterExprOp::Mul, left, right),
            SelectExpr::Div { left, right } => binary_op(FilterExprOp::Div, left, right),
            SelectExpr::Field { path } => FilterValue::FieldRef { path: path.clone() },
            SelectExpr::Literal { value } => match value {
                SelectExprValue::Null => FilterValue::Null,
                SelectExprValue::Bool(b) => FilterValue::Bool(*b),
                SelectExprValue::Int(i) => FilterValue::Int(*i),
                SelectExprValue::Float(f) => FilterValue::Float(*f),
                SelectExprValue::String(s) => FilterValue::String(s.clone()),
            },
        }
    }
}

/// Build a `FilterValue::Expr` node for a binary arithmetic `SelectExpr`
/// variant — shared by the `Add`/`Sub`/`Mul`/`Div` arms of
/// [`SelectExpr::to_filter_value`].
fn binary_op(op: FilterExprOp, left: &SelectExpr, right: &SelectExpr) -> FilterValue {
    FilterValue::Expr {
        expr: FilterExpr::new(op, vec![left.to_filter_value(), right.to_filter_value()]),
    }
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
