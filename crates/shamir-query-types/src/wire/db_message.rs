//! `DbRequest` / `DbResponse` — the application-layer payload of
//! `RequestEnvelope.req` / `ResponseEnvelope.res` after the SCRAM
//! handshake completes.

use serde::{Deserialize, Serialize};

use crate::batch::{BatchRequest, BatchResponse, TransactionInfo};
use crate::wire::repl::{ReplRequest, ReplResponse};

/// Current query-language version. Bumped when the on-the-wire
/// `BatchRequest` schema changes incompatibly. The server keeps a
/// hardcoded supported list (`SUPPORTED_QUERY_LANG_VERSIONS`) and
/// rejects unknown versions before invoking the DB layer.
///
/// v2: server now supports MessagePack id-keyed write/read pass-through.
/// Advertised to the client via `WireAuthOk.server_query_version` /
/// `WireResumeOkWire.server_query_version`; clients opt in only when
/// they see `server_query_version >= 2`.
pub const CURRENT_QUERY_LANG_VERSION: u32 = 2;

fn default_query_version() -> u32 {
    CURRENT_QUERY_LANG_VERSION
}

/// Application-layer DB request (msgpack-encoded payload of
/// `RequestEnvelope.req`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DbRequest {
    /// Health check — no DB lookup, no version negotiation.
    Ping,
    /// Execute a [`BatchRequest`] against the named database. The batch
    /// payload is forwarded verbatim to `ShamirDb::execute`; the full
    /// [`BatchResponse`] (records, stats, pagination, plan, transaction
    /// info) is returned to the client.
    Execute {
        /// Query-language version. Default: [`CURRENT_QUERY_LANG_VERSION`].
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name.
        db: String,
        /// Batch payload — see [`crate::batch::BatchRequest`].
        batch: BatchRequest,
    },
    /// Create a SCRAM-authenticatable user (the kind that can log in
    /// over the wire). Distinct from `BatchOp::CreateUser` (DB-level
    /// user for table permissions). Server runs Argon2id with its
    /// configured KDF defaults and writes the record to the durable
    /// user directory. Requires superuser session.
    CreateScramUser {
        /// Username (will be NFC + UsernameCaseMapped normalised on
        /// the server write path).
        name: String,
        /// Plaintext password. Hashed server-side; the server wraps the
        /// received `String` in `Zeroizing<Vec<u8>>` before deriving keys
        /// so it is zeroized on drop. The `String` here is the on-the-wire
        /// carrier — callers should avoid retaining it longer than needed.
        password: String,
        /// Roles to grant. `["superuser"]` for admin powers; other
        /// strings are opaque to the protocol (RBAC is app-defined).
        #[serde(default)]
        roles: Vec<String>,
    },

    // --- Phase B: interactive (multi-call) transactions ---
    /// Open an interactive transaction scoped to a single `repo`. The server
    /// mints an opaque `tx_handle`, opens an MVCC snapshot, and parks the
    /// live transaction bound to this session. Subsequent `TxExecute`s run
    /// against it until `TxCommit`/`TxRollback`. Cross-repo transactions stay
    /// out of scope (one repo per handle).
    TxBegin {
        /// Query-language version. Default: [`CURRENT_QUERY_LANG_VERSION`].
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name.
        db: String,
        /// The single repo the transaction is scoped to.
        repo: String,
        /// `"snapshot"` (default) | `"serializable"` — same vocabulary as
        /// [`crate::batch::BatchRequest::isolation`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        isolation: Option<String>,
    },
    /// Run a batch inside an already-open interactive transaction. The tx is
    /// NOT committed — state accumulates in the parked transaction. The
    /// batch's `transactional` flag is ignored here (the handle already
    /// establishes tx mode).
    TxExecute {
        /// Query-language version. Default: [`CURRENT_QUERY_LANG_VERSION`].
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name (must match the handle's pinned db).
        db: String,
        /// The handle minted by [`DbRequest::TxBegin`].
        tx_handle: u64,
        /// Batch payload — see [`crate::batch::BatchRequest`].
        batch: BatchRequest,
    },
    /// Commit an open interactive transaction (runs the full commit
    /// pipeline). The reply carries the [`TransactionInfo`].
    TxCommit {
        /// Target database name.
        db: String,
        /// The handle to commit.
        tx_handle: u64,
    },
    /// Roll back (abort) an open interactive transaction. Drops the parked
    /// transaction — RAII rollback, no storage side effects.
    TxRollback {
        /// Target database name.
        db: String,
        /// The handle to roll back.
        tx_handle: u64,
    },
    /// Privileged replication request (leader-facing). Carries the
    /// independently-versioned replication sub-protocol (REPLICATION §5,
    /// PR5) — keeps the client `DbRequest` surface flat while letting
    /// replication evolve without bumping the query-language version.
    /// R0: only `Hello` + `Pull` (§5.3).
    Repl(ReplRequest),
}

/// Application-layer DB response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DbResponse {
    /// Reply to [`DbRequest::Ping`].
    Pong,
    /// Successful batch execution.
    Batch {
        /// Full [`BatchResponse`] — no fields dropped.
        response: BatchResponse,
    },
    /// Successful [`DbRequest::CreateScramUser`].
    UserCreated {
        /// Echoed user name (post-normalisation).
        name: String,
        /// Stable 16-byte user_id assigned by the directory.
        #[serde(with = "serde_bytes")]
        user_id: Vec<u8>,
    },
    /// DB-layer failure (permission, planner, query, lock-timeout, …).
    /// Not a protocol error; the wire frame is a normal `ResponseEnvelope`.
    Error {
        /// Coarse classification of the failure. The full vocabulary actually
        /// emitted by the server (see `shamir-server` `db_handler`):
        ///
        /// - Auth / routing: `permission_denied`, `access_denied`,
        ///   `read_only_replica`, `bad_role`, `unsupported_query_version`.
        /// - Destructive-op HMAC gate: `hmac_required`, `hmac_mismatch`.
        /// - Batch / planner: `limits`, `validation`, `query`, `timeout`,
        ///   `lock_timeout`, `nesting_too_deep`, `unknown_db`, `not_supported`,
        ///   `user_exists`.
        /// - Interactive transactions: `tx_conflict`, `tx_not_found`,
        ///   `tx_already_open`, `tx_in_tx`, `tx_forbidden`, `tx_write`,
        ///   `tx_wrong_db`, `tx_wrong_repo`, `tx_cross_repo_not_supported`,
        ///   `tx_too_large`, `tx_error`.
        /// - Foreign keys: `fk_violation`, `fk_restrict`, `fk_cascade_depth`,
        ///   `fk_requires_index`, `fk_actions`, `fk_on_update`,
        ///   `fk_restrict`, `fk_update_unsupported_new_value`.
        ///
        /// A `QueryError` may also carry a structured `code` verbatim (e.g.
        /// `bad_hmac`, `fk_*`); untagged legacy errors fall back to `query`.
        code: String,
        /// Human-readable detail.
        message: String,
    },

    // --- Phase B: interactive (multi-call) transactions ---
    /// Reply to [`DbRequest::TxBegin`] — the minted handle + the snapshot
    /// version the transaction reads at.
    TxOpened {
        /// Opaque handle for subsequent `TxExecute`/`TxCommit`/`TxRollback`.
        tx_handle: u64,
        /// MVCC version the transaction's snapshot reads at.
        snapshot_version: u64,
        /// Effective isolation (`"snapshot"` | `"serializable"`).
        isolation: String,
    },
    /// Reply to [`DbRequest::TxExecute`] — the same [`BatchResponse`] a non-tx
    /// `Execute` returns, except `BatchResponse.transaction` stays `None`
    /// (the tx is still open; there is no per-call commit outcome yet).
    TxBatch {
        /// Full [`BatchResponse`] for this call.
        response: BatchResponse,
    },
    /// Reply to [`DbRequest::TxCommit`] — the commit outcome, produced at
    /// COMMIT time rather than per batch.
    TxCommitted {
        /// Committed-or-aborted transaction info.
        transaction: TransactionInfo,
    },
    /// Reply to [`DbRequest::TxRollback`].
    TxRolledBack {
        /// The handle that was rolled back.
        tx_handle: u64,
    },
    /// Privileged replication reply (mirrors [`DbRequest::Repl`]). The
    /// nested [`ReplResponse`] carries `leader_epoch` on every variant for
    /// VR-style fencing (§5.2).
    Repl(ReplResponse),
}
