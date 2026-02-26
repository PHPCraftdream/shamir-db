//! Select types for query projections.
//!
//! Supports fields, aggregations, aliases, and expressions.

use serde::{Deserialize, Serialize};

use crate::db::query::filter::FieldPath;

/// What to select/return from a query
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Select {
    /// Select items (fields, aggregations, expressions)
    pub items: Vec<SelectItem>,
    /// Return distinct results
    #[serde(default)]
    pub distinct: bool,
}

impl Select {
    /// Select all fields (SELECT *)
    pub fn all() -> Self {
        Select {
            items: vec![SelectItem::All],
            distinct: false,
        }
    }

    /// Select specific fields
    pub fn fields(fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Select {
            items: fields
                .into_iter()
                .map(|f| SelectItem::Field {
                    path: f.into(),
                    alias: None,
                })
                .collect(),
            distinct: false,
        }
    }

    /// Add distinct modifier
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }
}

/// Single select item
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SelectItem {
    /// Select all fields (*)
    All,

    /// Select a field with optional alias
    Field {
        path: FieldPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },

    /// Aggregation function
    Aggregate {
        func: AggFunc,
        field: AggregateField,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
        #[serde(default)]
        distinct: bool,
    },

    /// Count all records
    CountAll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },

    /// Expression (future: computed fields)
    #[serde(rename = "expr")]
    Expression {
        expr: Expr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },
}

/// Aggregation functions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// Field for aggregation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AggregateField {
    /// Regular field
    Field(FieldPath),
    /// All fields (*)
    All,
}

/// Simple expressions (for future expansion)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Expr {
    // Arithmetic
    Add { left: Box<Expr>, right: Box<Expr> },
    Sub { left: Box<Expr>, right: Box<Expr> },
    Mul { left: Box<Expr>, right: Box<Expr> },
    Div { left: Box<Expr>, right: Box<Expr> },

    // Field reference
    Field { path: FieldPath },

    // Literal value
    Literal { value: ExprValue },
}

/// Expression values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExprValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}
