//! Filter types for WHERE, HAVING, UPDATE, DELETE clauses.
//!
//! Supports comparison, logical, and array operators.

use serde::{Deserialize, Serialize};

/// Field path (e.g., "user.email" or "tags")
pub type FieldPath = String;

/// A complete filter expression (WHERE/HAVING)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Filter {
    // Comparison operators
    Eq {
        field: FieldPath,
        value: FilterValue,
    },
    Ne {
        field: FieldPath,
        value: FilterValue,
    },
    Gt {
        field: FieldPath,
        value: FilterValue,
    },
    Gte {
        field: FieldPath,
        value: FilterValue,
    },
    Lt {
        field: FieldPath,
        value: FilterValue,
    },
    Lte {
        field: FieldPath,
        value: FilterValue,
    },

    // Pattern matching
    Like {
        field: FieldPath,
        pattern: String,
    },
    ILike {
        field: FieldPath,
        pattern: String,
    },
    Regex {
        field: FieldPath,
        pattern: String,
    },

    // Null checks
    IsNull {
        field: FieldPath,
    },
    IsNotNull {
        field: FieldPath,
    },

    // Array/containment operators
    In {
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    NotIn {
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    Contains {
        field: FieldPath,
        value: FilterValue,
    },
    ContainsAny {
        field: FieldPath,
        values: Vec<FilterValue>,
    },
    ContainsAll {
        field: FieldPath,
        values: Vec<FilterValue>,
    },

    // Range
    Between {
        field: FieldPath,
        from: FilterValue,
        to: FilterValue,
    },

    // Existence
    Exists {
        field: FieldPath,
    },
    NotExists {
        field: FieldPath,
    },

    // Logical operators
    And {
        filters: Vec<Filter>,
    },
    Or {
        filters: Vec<Filter>,
    },
    Not {
        filter: Box<Filter>,
    },

    // Shortcut: field equals value
    #[serde(rename = "field")]
    FieldEq {
        field: FieldPath,
        value: FilterValue,
    },
}

/// Value types supported in filters
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<FilterValue>),
    /// Reference to another field in the same document
    FieldRef {
        #[serde(rename = "$ref")]
        path: FieldPath,
    },
    /// Reference to another query's result in the same batch
    QueryRef {
        #[serde(rename = "$query")]
        alias: String,
        /// Optional path into the result
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// System function call ($fn)
    FnCall {
        #[serde(rename = "$fn")]
        call: FnCall,
    },
    /// Expression ($expr)
    Expr {
        #[serde(rename = "$expr")]
        expr: Expr,
    },
    /// Conditional ($cond)
    Cond {
        #[serde(rename = "$cond")]
        cond: Box<Cond>,
    },
}

impl FilterValue {
    pub fn is_null(&self) -> bool {
        matches!(self, FilterValue::Null)
    }

    /// Create a field reference
    pub fn field_ref(path: impl Into<String>) -> Self {
        FilterValue::FieldRef { path: path.into() }
    }

    /// Create a query reference (references another query's result in a batch)
    pub fn query_ref(alias: impl Into<String>) -> Self {
        FilterValue::QueryRef {
            alias: alias.into(),
            path: None,
        }
    }

    /// Create a query reference with a path
    pub fn query_ref_with_path(alias: impl Into<String>, path: impl Into<String>) -> Self {
        FilterValue::QueryRef {
            alias: alias.into(),
            path: Some(path.into()),
        }
    }
}

impl From<i64> for FilterValue {
    fn from(v: i64) -> Self {
        FilterValue::Int(v)
    }
}

impl From<f64> for FilterValue {
    fn from(v: f64) -> Self {
        FilterValue::Float(v)
    }
}

impl From<bool> for FilterValue {
    fn from(v: bool) -> Self {
        FilterValue::Bool(v)
    }
}

impl From<String> for FilterValue {
    fn from(v: String) -> Self {
        FilterValue::String(v)
    }
}

impl From<&str> for FilterValue {
    fn from(v: &str) -> Self {
        FilterValue::String(v.to_string())
    }
}

impl<T: Into<FilterValue>> From<Vec<T>> for FilterValue {
    fn from(v: Vec<T>) -> Self {
        FilterValue::Array(v.into_iter().map(|x| x.into()).collect())
    }
}

// ============================================================================
// SYSTEM FUNCTIONS ($fn)
// ============================================================================

/// System function call ($fn).
///
/// Supports both simple (no args) and complex (with args) forms.
///
/// # Examples
///
/// ```json
/// // Simple (no args)
/// { "$fn": "NOW" }
/// { "$fn": "UUID" }
///
/// // With args
/// { "$fn": { "name": "COALESCE", "args": [null, "default"] } }
/// { "$fn": { "name": "SUBSTRING", "args": [{ "$ref": "name" }, 0, 10] } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FnCall {
    /// Simple form: just function name (no arguments)
    Simple(String),
    /// Complex form: name + arguments
    Complex {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<FilterValue>,
    },
}

impl FnCall {
    /// Create a simple function call (no args)
    pub fn simple(name: impl Into<String>) -> Self {
        FnCall::Simple(name.into())
    }

    /// Create a complex function call with args
    pub fn complex(name: impl Into<String>, args: Vec<FilterValue>) -> Self {
        FnCall::Complex {
            name: name.into(),
            args,
        }
    }

    /// Get the function name
    pub fn name(&self) -> &str {
        match self {
            FnCall::Simple(name) => name,
            FnCall::Complex { name, .. } => name,
        }
    }

    /// Get the arguments (empty for simple form)
    pub fn args(&self) -> &[FilterValue] {
        match self {
            FnCall::Simple(_) => &[],
            FnCall::Complex { args, .. } => args,
        }
    }
}

// ============================================================================
// EXPRESSIONS ($expr)
// ============================================================================

/// Expression operator for $expr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExprOp {
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
/// ```json
/// { "$expr": { "op": "add", "args": [10, 20] } }
/// { "$expr": { "op": "mul", "args": [{ "$ref": "price" }, 1.1] } }
/// { "$expr": { "op": "concat", "args": [{ "$ref": "first" }, " ", { "$ref": "last" }] } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub op: ExprOp,
    pub args: Vec<FilterValue>,
}

impl Expr {
    /// Create a new expression
    pub fn new(op: ExprOp, args: Vec<FilterValue>) -> Self {
        Expr { op, args }
    }

    /// Create an add expression
    pub fn add(args: Vec<FilterValue>) -> Self {
        Expr::new(ExprOp::Add, args)
    }

    /// Create a mul expression
    pub fn mul(args: Vec<FilterValue>) -> Self {
        Expr::new(ExprOp::Mul, args)
    }

    /// Create a concat expression
    pub fn concat(args: Vec<FilterValue>) -> Self {
        Expr::new(ExprOp::Concat, args)
    }
}

// ============================================================================
// CONDITIONS ($cond)
// ============================================================================

/// Conditional ($cond) - ternary operator.
///
/// Returns `then` if condition is true, otherwise `else`.
/// The `if` field uses the existing Filter syntax.
///
/// # Examples
///
/// ```json
/// {
///   "$cond": {
///     "if": { "op": "eq", "field": "active", "value": true },
///     "then": "yes",
///     "else": "no"
///   }
/// }
/// ```
///
/// Nested conditions:
/// ```json
/// {
///   "$cond": {
///     "if": { "op": "gte", "field": "score", "value": 100 },
///     "then": "vip",
///     "else": {
///       "$cond": {
///         "if": { "op": "gte", "field": "score", "value": 50 },
///         "then": "regular",
///         "else": "newbie"
///       }
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cond {
    /// Condition (uses Filter syntax)
    #[serde(rename = "if")]
    pub condition: Box<Filter>,
    /// Value if condition is true
    pub then: FilterValue,
    /// Value if condition is false
    #[serde(rename = "else")]
    pub or_else: FilterValue,
}

impl Cond {
    /// Create a new conditional
    pub fn new(condition: Filter, then: FilterValue, or_else: FilterValue) -> Self {
        Cond {
            condition: Box::new(condition),
            then,
            or_else,
        }
    }
}
