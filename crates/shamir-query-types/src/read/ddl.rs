//! DDL operation status types for the unified DDL result contract (#1015).

use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;

/// Full status of a DDL operation, queryable via the
/// `DbRequest::GetDdlOpStatus` poll endpoint.
///
/// Correlates to a specific `op_id` minted at dispatch time and embedded
/// in the synchronous `QueryResult` for the operation (as `QueryResult::op_id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DdlOpStatus {
    /// The operation's stable identifier (minted by the server at dispatch).
    pub op_id: RecordId,
    /// Kind of DDL operation that was performed.
    pub kind: DdlOpKind,
    /// Current state of the operation.
    pub state: DdlOpState,
}

/// Classification of which DDL operation family this is.
///
/// Used for both client display and for validation that a poll request
/// is operating on the right operation type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DdlOpKind {
    /// `CREATE INDEX` (regular hash family).
    CreateHashIndex {
        /// Index name.
        index_name: String,
        /// Table the index is on.
        table_name: String,
    },
    /// `CREATE INDEX` (unique hash family).
    CreateUniqueHashIndex {
        /// Index name.
        index_name: String,
        /// Table the index is on.
        table_name: String,
    },
    /// `DROP INDEX` (regular hash family).
    DropHashIndex {
        /// Index name.
        index_name: String,
    },
    /// `DROP INDEX` (unique hash family).
    DropUniqueHashIndex {
        /// Index name.
        index_name: String,
    },
    /// `RENAME INDEX` (regular hash family).
    RenameHashIndex {
        /// Old index name.
        old_name: String,
        /// New index name.
        new_name: String,
    },
    /// `RENAME INDEX` (unique hash family).
    RenameUniqueHashIndex {
        /// Old index name.
        old_name: String,
        /// New index name.
        new_name: String,
    },
    /// `DROP INDEX` (index2 family).
    DropIndex2 {
        /// Index name.
        index_name: String,
    },
    /// Placeholder for other DDL operations not yet wired in this slice
    /// (e.g., CREATE/DROP TABLE, CREATE/DROP DATABASE, etc.).
    Other {
        /// Human-readable description of the operation.
        description: String,
    },
}

/// State of a DDL operation.
///
/// See RFC §2.4 for the precise semantics of each variant and who writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DdlOpState {
    /// Operation started, not yet durably complete.
    ///
    /// Written by the dispatch handler before the first mutating step.
    InProgress,
    /// Completed successfully on the synchronous (non-crash) path.
    ///
    /// Written by the normal success path, after the operation is durably
    /// complete and before the tombstone is cleared.
    Succeeded {
        /// Timestamp (milliseconds since UNIX epoch) when the op succeeded.
        completed_at: u64,
    },
    /// Completed successfully via crash-recovery on server restart.
    ///
    /// Written by the recovery function (`recover_hash_renames`,
    /// `recover_index2_drops`, `recover_in_progress_drops`) as it clears
    /// each tombstone, using the SAME `op_id` that was in the tombstone
    /// payload. Distinguishable from `Succeeded` so an operator knows a
    /// restart occurred and any in-flight writers at crash time did not
    /// see the constraint transiently.
    SucceededViaCrashRecovery {
        /// Timestamp (milliseconds since UNIX epoch) when the op completed
        /// via recovery.
        completed_at_restart: u64,
    },
    /// Operation failed and was NOT recovered.
    ///
    /// Written at the existing enriched-error sites (replaces the free-text
    /// `DbError::Internal` workaround from #967). The client may need to
    /// call `TableManager::verify()`/`repair()` to recover.
    Failed {
        /// Human-readable failure detail.
        detail: String,
    },
    /// Operation ID not found (GC'd, never existed, or a pre-RFC op).
    ///
    /// This is the default returned by the poll endpoint for an unknown
    /// `op_id`; it is never persisted.
    Unknown,
}
