//! Aggregation types — AggFunc and AggregateField.

use serde::{Deserialize, Serialize};

use crate::filter::FieldPath;

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
