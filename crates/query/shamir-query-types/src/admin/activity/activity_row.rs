//! [`ActivityRow`] and its state enums (R4b step 1, §5.1/§2).
//!
//! Privacy rule (D3): rows carry ids, actor, db, op kind, timing and state
//! only — never filter values, literals, row data, message bodies, the
//! bearer token or its prefix.

use serde::{Deserialize, Serialize};

/// One rendered activity-view row; tagged by `kind` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityRow {
    /// A live client session (identity is the minted numeric handle, D3).
    Session {
        /// Minted, non-secret session handle.
        session_handle: u64,
        /// Authenticated user name.
        user: String,
        /// Transport name (e.g. `tcp`, `http`, `ipc`).
        transport: String,
        /// Session creation timestamp, nanoseconds since the epoch.
        created_at_ns: u64,
        /// Nanoseconds since the last activity on the session.
        idle_ns: u64,
        /// Interactive transactions currently open on the session.
        open_transactions: usize,
        /// Cursors currently open on the session.
        open_cursors: usize,
    },
    /// An in-flight batch execution (server-minted id, D5).
    Request {
        /// Minted server-side execution id (not the optional client `rid`).
        request_exec_id: RequestExecId,
        /// Owning session handle (`None` for stateless REST requests,
        /// which are not backed by a stored session and therefore have
        /// no session handle at all; omitted on the wire, defaulted
        /// back to `None` when absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_handle: Option<u64>,
        /// Authenticated user name.
        user: String,
        /// Target database name.
        db: String,
        /// Operation-kind summary (kind NAMES only: `read` / `write` /
        /// `tx_open` / `admin:<op>`; derived from the parsed discriminant,
        /// never from arguments).
        op_kind: String,
        /// Execution state.
        state: ActivityRequestState,
        /// Nanoseconds since the request was admitted.
        elapsed_ns: u64,
    },
    /// An interactive transaction.
    Transaction {
        /// Transaction handle.
        tx_handle: u64,
        /// Owning session handle.
        session_handle: u64,
        /// Authenticated user name.
        user: String,
        /// Transaction state.
        state: ActivityTxState,
        /// Nanoseconds since the last activity on the transaction.
        idle_ns: u64,
    },
    /// An open cursor.
    Cursor {
        /// Cursor id minted by the server.
        cursor_id: u64,
        /// Owning session handle.
        session_handle: u64,
        /// Authenticated user name.
        user: String,
        /// Table NAME only (no filter values or literals).
        table: String,
        /// Snapshot version the cursor is pinned to.
        pinned_version: u64,
        /// Nanoseconds since the cursor was opened.
        open_ns: u64,
    },
}

/// State of an in-flight request execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityRequestState {
    /// Admitted, still executing.
    Running,
    /// Finished (committed or returned); about to be deregistered.
    Done,
    /// Entered commit; no longer cancellable.
    Committing,
    /// A cancellation request won before commit.
    Cancelled,
    /// Parked waiting for a record lock.
    WaitingLock,
    /// Parked waiting for a visibility position.
    WaitingVisibility,
    /// Parked waiting for byte-budget capacity.
    WaitingBudget,
}

/// State of an interactive transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityTxState {
    /// Open and executing statements.
    Active,
    /// Waiting for the client's commit decision.
    AwaitingCommit,
    /// Idle past its lease; pending reaper collection.
    ExpiredPendingReap,
}

/// Server-minted batch-execution identifier (D5).
///
/// Minted per admitted BATCH by the server's db handler; the client `rid`
/// stays optional and is carried alongside, not replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestExecId(pub u64);
