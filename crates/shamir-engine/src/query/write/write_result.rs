//! Write operation result.

use serde::{Deserialize, Serialize};

/// Result of a write operation (insert, update, set, delete).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteResult {
    /// Number of records affected by the operation.
    pub affected: u64,
    /// Returned records (if requested via UpdateSelect, or inserted records).
    pub records: Vec<serde_json::Value>,
    /// Execution time in microseconds.
    pub execution_time_us: u64,
}
