//! [`ShapeStatsRow`] — one canonical shape key and its counters (R4b step 4).
//!
//! Privacy rule: the key is a hash, so a row carries no field names, no
//! filter values, no literals, no user identity and no table name.

use serde::{Deserialize, Serialize};

/// One per-shape statistics row: the canonical shape key as 32 lowercase hex
/// characters plus its counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeStatsRow {
    /// Canonical shape key as 32 lowercase hex characters.
    pub shape_hex: String,
    /// Number of executions attributed to this shape.
    pub executions: u64,
    /// Summed wall time over those executions, in nanoseconds.
    pub total_nanos: u64,
    /// Number of those executions that failed.
    pub errors: u64,
}
