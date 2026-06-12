//! [`BatchRequest`] — the wire DTO for a batch of operations.

use serde::{Deserialize, Serialize};
use shamir_collections::TMap;

use super::batch_limits::BatchLimits;
use super::query_entry::QueryEntry;

/// Batch request containing multiple queries.
///
/// # JSON Format
///
/// ```json
/// {
///   "name": "my_batch",
///   "transactional": false,
///   "queries": {
///     "users": { "from": "users" },
///     "orders": {
///       "query": { "from": "orders" },
///       "return_result": false
///     }
///   },
///   "return_all": true,
///   "return_only": ["users"],
///   "limits": { ... }
/// }
/// ```
///
/// # Fields
///
/// - `name`: Optional name for logging/debugging
/// - `transactional`: Enable MVCC transaction semantics
/// - `queries`: Map of alias -> query entry
/// - `return_all`: Return all results (default: true)
/// - `return_only`: Specific aliases to return (overrides return_all)
/// - `limits`: Security limits
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRequest {
    /// Client-provided request ID, echoed back in the response.
    /// Used for correlating async requests with responses.
    pub id: serde_json::Value,

    /// Optional name for logging/debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Enable transactional semantics (MVCC).
    ///
    /// When true, all queries see a consistent snapshot.
    #[serde(default)]
    pub transactional: bool,

    /// Requested isolation level for transactional batches.
    ///
    /// - `"snapshot"` (default) — Snapshot Isolation. Reads see a
    ///   consistent snapshot; writes use last-writer-wins.
    /// - `"serializable"` — Serializable Snapshot Isolation. Read-set
    ///   validated at commit; concurrent write conflict → abort.
    ///
    /// Ignored when `transactional` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,

    /// Per-request durability level.
    ///
    /// - `"buffered"` (default / absent) — ack after the in-memory
    ///   MemBuffer; durability on the ~500 ms background tick or
    ///   graceful drain.
    /// - `"synced"` — before ack, flush the durable backing of every
    ///   repo this batch touched, so a committed write survives even
    ///   an immediate hard crash.
    /// - `"async_index"` — ack after WAL fsync + data apply + MVCC publish;
    ///   index posting apply, recovery markers, WAL cleanup, and HNSW promote
    ///   run on a background task. Shortens the pre-ACK critical section while
    ///   preserving WAL durability and read-your-own-writes on data. Only
    ///   meaningful for `transactional: true` batches; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<String>,

    /// Queries map: alias -> query entry.
    ///
    /// Each key is the alias used in `$query` references.
    /// The value can be just a `Query` or a `QueryEntry` with options.
    pub queries: TMap<String, QueryEntry>,

    /// Return all results (default: true).
    #[serde(default = "default_return_all")]
    pub return_all: bool,

    /// Specific aliases to return (overrides return_all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_only: Option<Vec<String>>,

    /// Execution limits (security).
    #[serde(default = "BatchLimits::default")]
    pub limits: BatchLimits,
}

fn default_return_all() -> bool {
    true
}
