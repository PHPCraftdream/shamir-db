//! [`BatchResponse`] — the wire DTO returned after executing a batch.

use serde::{Deserialize, Serialize};
use shamir_collections::TMap;

use crate::read::QueryResult;

use super::transaction_info::TransactionInfo;

/// Batch response with results.
///
/// # JSON Format
///
/// ```json
/// {
///   "results": {
///     "users": [...],
///     "orders": [...]
///   },
///   "execution_plan": [["users", "products"], ["orders"], ["stats"]],
///   "execution_time_us": 1234,
///   "transaction": { "id": 1, "committed": true }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchResponse {
    /// Echoed request ID from BatchRequest.
    pub id: serde_json::Value,

    /// Results by alias.
    #[serde(default)]
    pub results: TMap<String, QueryResult>,

    /// Execution plan (for debugging).
    ///
    /// Each inner array contains queries that run in parallel.
    pub execution_plan: Vec<Vec<String>>,

    /// Total execution time in microseconds.
    pub execution_time_us: u64,

    /// Transaction info (if transactional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionInfo>,
}
