//! [`BatchRequest`] — the wire DTO for a batch of operations.

use serde::{Deserialize, Serialize};
use shamir_collections::TMap;
use shamir_types::types::value::QueryValue;

use crate::batch::batch_limits::BatchLimits;
use crate::batch::options::{
    deserialize_optional_field, IndexVisibility, IsolationLevel, RequestDurability,
};
use crate::batch::query_entry::QueryEntry;
use crate::batch::result_encoding::ResultEncoding;

/// Batch request containing multiple queries.
///
/// # Wire Format
///
/// ```text
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
/// - `isolation`: `snapshot|serializable|pessimistic` (typed enum, v4)
/// - `durability`: `buffered|synced` (typed enum, v4); the old
///   `"async_index"` string moved to `index_visibility: sync|async`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    /// Client-provided request ID, echoed back in the response.
    /// Used for correlating async requests with responses.
    pub id: QueryValue,

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
    /// - [`IsolationLevel::Snapshot`] (default) — Snapshot Isolation. Reads
    ///   see a consistent snapshot; writes use last-writer-wins.
    /// - [`IsolationLevel::Serializable`] — Serializable Snapshot Isolation.
    ///   Read-set validated at commit; concurrent write conflict → abort.
    /// - [`IsolationLevel::Pessimistic`] — level-3 locks guard concurrent
    ///   writers.
    ///
    /// Strictly typed (v4): unknown wire strings are rejected. Ignored when
    /// `transactional` is false.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_isolation"
    )]
    pub isolation: Option<IsolationLevel>,

    /// Per-request durability level (typed enum, v4).
    ///
    /// - [`RequestDurability::Buffered`] (default / absent) — ack after the
    ///   buffered WAL append; periodic fsync runs nominally every 250 ms
    ///   (not a hard bound), or on graceful drain.
    /// - [`RequestDurability::Synced`] — before ack, flush the durable
    ///   backing of every repo this batch touched, so a committed write
    ///   survives even an immediate hard crash.
    ///
    /// The old `"async_index"` value is REMOVED; the index-apply visibility
    /// it controlled moved to [`BatchRequest::index_visibility`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_durability"
    )]
    pub durability: Option<RequestDurability>,

    /// Index-apply visibility for transactional commits (NEW in v4).
    ///
    /// - [`IndexVisibility::Sync`] (default / absent) — index posting apply
    ///   runs inline before ack.
    /// - [`IndexVisibility::Async`] — ack after WAL fsync + data apply +
    ///   MVCC publish; index posting apply, recovery markers, WAL cleanup,
    ///   and HNSW promote run on a background task. Replaces the old
    ///   `durability: "async_index"` string. Only meaningful for
    ///   `transactional: true` batches; ignored otherwise.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_index_visibility"
    )]
    pub index_visibility: Option<IndexVisibility>,

    /// Commit-only override; VIS-1 carries it without enabling bounded waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_wait: Option<super::VisibilityWait>,

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

    /// Per-repo interner epochs the client has cached (Stage 5-wire Part A).
    ///
    /// Keyed by repo name; value is the client's current gap-free high-water
    /// epoch for that repo's interner. The server attaches a per-repo
    /// [`InternerDelta`](super::interner_delta::InternerDelta) to the response
    /// for every entry here. Backward-compatible: `#[serde(default)]` → old
    /// clients / servers that don't know the field leave it empty.
    #[serde(default, skip_serializing_if = "TMap::is_empty")]
    pub interner_epochs: TMap<String, u64>,

    /// Encoding for result rows returned in the response.
    ///
    /// Name (default, legacy) = server de-interns rows to name-keyed
    /// [`QueryValue`](shamir_types::types::value::QueryValue); Id = server
    /// returns id-keyed [`QueryRecord::IdBytes`](crate::read::QueryRecord),
    /// client de-interns. Backward-compatible: absent → [`ResultEncoding::Name`].
    #[serde(default)]
    pub result_encoding: ResultEncoding,
}

fn deserialize_durability<'de, D>(deserializer: D) -> Result<Option<RequestDurability>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_field(deserializer, "durability")
}

fn deserialize_isolation<'de, D>(deserializer: D) -> Result<Option<IsolationLevel>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_field(deserializer, "isolation")
}

fn deserialize_index_visibility<'de, D>(
    deserializer: D,
) -> Result<Option<IndexVisibility>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_field(deserializer, "index_visibility")
}

fn default_return_all() -> bool {
    true
}
