//! [`ShapeStatsTotals`] — the `(other)` fold totals (R4b step 4).

use serde::{Deserialize, Serialize};

/// Totals for the `(other)` fold: rows evicted at the registry cap and
/// executions that could not reserve a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeStatsTotals {
    /// Folded execution count.
    pub executions: u64,
    /// Folded wall time, in nanoseconds.
    pub total_nanos: u64,
    /// Folded error count.
    pub errors: u64,
}
