//! Cond — conditional ($cond) ternary operator.

use serde::{Deserialize, Serialize};

use super::{Filter, FilterValue};

/// Conditional ($cond) - ternary operator.
///
/// Returns `then` if condition is true, otherwise `else`.
/// The `if` field uses the existing Filter syntax.
///
/// # Examples
///
/// ```text
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
/// ```text
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
