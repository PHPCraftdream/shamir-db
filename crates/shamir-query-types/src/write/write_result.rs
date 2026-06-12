//! Write operation result.

use serde::{Deserialize, Serialize};

use super::InsertedRecord;

/// Result of a write operation (insert, update, set, delete).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteResult {
    /// Number of records affected by the operation.
    pub affected: u64,
    /// Returned records (if requested via UpdateSelect, or inserted records).
    ///
    /// The element type is [`InsertedRecord`] rather than `serde_json::Value`
    /// so that INSERT hot paths can skip the `serde_json::Map` allocation
    /// (`Direct` variant). Wire bytes are identical — both variants emit the
    /// same msgpack map shape.
    pub records: Vec<InsertedRecord>,
    /// Execution time in microseconds.
    pub execution_time_us: u64,
}
