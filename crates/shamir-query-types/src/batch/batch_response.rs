//! [`BatchResponse`] — the wire DTO returned after executing a batch.

use serde::{Deserialize, Serialize};
use shamir_collections::TMap;
use shamir_types::types::value::QueryValue;

use crate::read::QueryResult;

use super::interner_delta::InternerDelta;
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
    pub id: QueryValue,

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

    /// Per-repo interner deltas for ambient cache sync (Stage 5-wire Part A).
    ///
    /// Keyed by repo name; present only for repos the client advertised an
    /// epoch for in [`BatchRequest::interner_epochs`](super::BatchRequest::interner_epochs).
    /// Backward-compatible: `#[serde(default, skip_serializing_if = "is_empty")]`
    /// → old peers never see the field.
    #[serde(default, skip_serializing_if = "TMap::is_empty")]
    pub interner_delta: TMap<String, InternerDelta>,
}
