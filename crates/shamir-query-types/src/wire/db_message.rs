//! `DbRequest` / `DbResponse` — the application-layer payload of
//! `RequestEnvelope.req` / `ResponseEnvelope.res` after the SCRAM
//! handshake completes.

use serde::{Deserialize, Serialize};

use crate::batch::{BatchRequest, BatchResponse};

/// Current query-language version. Bumped when the on-the-wire
/// `BatchRequest` schema changes incompatibly. The server keeps a
/// hardcoded supported list (`SUPPORTED_QUERY_LANG_VERSIONS`) and
/// rejects unknown versions before invoking the DB layer.
pub const CURRENT_QUERY_LANG_VERSION: u32 = 1;

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
        /// Plaintext password. Hashed server-side; zeroized after.
        password: String,
        /// Roles to grant. `["superuser"]` for admin powers; other
        /// strings are opaque to the protocol (RBAC is app-defined).
        #[serde(default)]
        roles: Vec<String>,
    },
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
        /// Coarse classification: `permission_denied`, `validation`,
        /// `limits`, `query`, `timeout`, `lock_timeout`, `unknown_db`,
        /// `not_supported`, `user_exists`.
        code: String,
        /// Human-readable detail.
        message: String,
    },
}
