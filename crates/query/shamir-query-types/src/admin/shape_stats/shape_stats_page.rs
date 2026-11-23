//! [`ShapeStatsPage`] — the `ListShapeStats` response page (R4b step 4).

use crate::admin::{ShapeStatsRow, ShapeStatsTotals};
use serde::{Deserialize, Serialize};

/// One bounded page of per-shape statistics for one repository.
///
/// Deterministic order: `total_nanos` desc, then `executions` desc, then
/// `shape_hex` asc. There is no continuation — the registry is capped at
/// 256 rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeStatsPage {
    /// Rendered rows (at most the caller's clamped `limit`).
    pub rows: Vec<ShapeStatsRow>,
    /// The `(other)` fold totals: rows evicted at the cap and executions
    /// that could not reserve a row.
    pub other: ShapeStatsTotals,
    /// Rows currently in the registry, excluding `(other)`.
    pub tracked: u32,
    /// `true` when the caller's `limit` cut the page.
    pub truncated: bool,
}
