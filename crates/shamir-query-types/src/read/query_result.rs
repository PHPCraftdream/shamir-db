//! QueryResult and QueryStats — query execution results.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

use super::query_record::QueryRecord;
use super::PaginationInfo;

/// Plan type chosen by the read planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanType {
    /// Keyset-seek via sorted index (Pagination::After).
    KeysetSeek,
    /// ORDER BY + LIMIT K fast path via sorted index.
    OrderLimitFast,
    /// Index2 accelerated path (FTS / functional / vector).
    Index2,
    /// BTree index equality / In scan.
    IndexScan,
    /// Sorted index range scan (Between / Gte / Lte / Gt / Lt).
    SortedIndexScan,
    /// Range predicate extracted from AND filter + sorted index.
    AndRangeIndexScan,
    /// Counter shortcut (COUNT(*) without WHERE).
    CounterShortcut,
    /// MIN/MAX aggregate via sorted index.
    MinMaxIndex,
    /// Full table scan (streaming / collecting / counting).
    FullScan,
}

/// EXPLAIN plan preview — returned when `ReadQuery::explain == true`.
///
/// Contains the plan the planner WOULD choose, without materialising
/// any rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainPlan {
    /// Which execution strategy the planner selected.
    pub plan_type: PlanType,
    /// Name of the index used (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_used: Option<String>,
    /// Estimated records to scan (when the planner knows before execution).
    /// `None` when the estimate is unavailable without materialisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_rows: Option<u64>,
}

/// Query execution statistics
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryStats {
    /// Was an index used?
    pub index_used: Option<String>,
    /// Number of records scanned
    pub records_scanned: u64,
    /// Number of records returned
    pub records_returned: u64,
    /// Execution time in microseconds
    pub execution_time_us: u64,
}

/// Query result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    /// Result records.
    pub records: Vec<QueryRecord>,
    /// Execution statistics
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<QueryStats>,
    /// Pagination metadata (present when pagination was used)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationInfo>,
    /// Non-tabular value returned by a stored procedure / callable function.
    ///
    /// When a `CallOp` returns a scalar, object, or array the answer lives
    /// here; `records` is empty. `None` for regular DML/DDL results.
    /// Skipped from serialization when absent (backward-compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<QueryValue>,
    /// EXPLAIN plan preview (present only when `ReadQuery::explain == true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<ExplainPlan>,
}
