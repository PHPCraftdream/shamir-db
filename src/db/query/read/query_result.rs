//! QueryResult and QueryStats — query execution results.

use serde::{Deserialize, Serialize};

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
    /// Result records (as JSON values)
    pub records: Vec<serde_json::Value>,
    /// Execution statistics
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<QueryStats>,
    /// Has more results (for pagination)
    #[serde(default)]
    pub has_more: bool,
}
