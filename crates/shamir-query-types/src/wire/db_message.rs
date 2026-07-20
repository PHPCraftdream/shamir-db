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
    /// user directory. Requires superuser session AND an HMAC
    /// confirmation tag (same "did-you-mean-it" mechanism as
    /// `SetSuperuser` and destructive `BatchOp`s, task #604 — gated
    /// inline in `create_scram_user`'s handler since this is a
    /// top-level `DbRequest`, not a `BatchOp`).
    CreateScramUser {
        /// Username (will be NFC + UsernameCaseMapped normalised on
        /// the server write path).
        name: String,
        /// Plaintext password. Hashed server-side; the server wraps the
        /// revealed cleartext in `Zeroizing<Vec<u8>>` before deriving keys
        /// so it is zeroized on drop. Wire-level the value is still a
        /// plain string (`SecretString`'s `Serialize`/`Deserialize` are a
        /// transparent pass-through — the on-the-wire shape is unchanged),
        /// but the deserialized field itself is now `SecretString` so it
        /// can no longer leak through `Debug`/logging before the
        /// server-side `Zeroizing` wrap happens.
        password: crate::auth::SecretString,
        /// Roles to grant. Other strings are opaque to the protocol
        /// (RBAC is app-defined). NOTE (task #557): the literal
        /// `"superuser"` string is RESERVED at the directory write boundary
        /// — supplying it here surfaces a `query`-class error from the
        /// server. Use [`DbRequest::SetSuperuser`] to grant admin powers.
        #[serde(default)]
        roles: Vec<String>,
        /// Hex-encoded HMAC-SHA256 tag over the canonical form — always
        /// required (unconditional, symmetric with `SetSuperuser`'s gate;
        /// task #604).
        hmac: Option<String>,
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

    /// Step 1 of `changePassword` (spec §12.5): the caller's own session
    /// requests a fresh server-side challenge (`server_nonce_cp`) bound to
    /// its own `session_id`. No permission gate beyond "you hold a valid
    /// session" — the caller can only ever change their own password
    /// (proven by the SCRAM proof-of-old-password in
    /// [`DbRequest::ChangePasswordVerify`]), so there is nothing extra to
    /// authorize here.
    ChangePasswordChallenge {
        /// Client-supplied per-request CSPRNG nonce (anti-replay).
        #[serde(with = "serde_bytes")]
        client_nonce_cp: Vec<u8>,
    },
    /// Step 2 of `changePassword` (spec §12.5): submit the SCRAM proof
    /// recovered from the OLD password plus the NEW credential material
    /// (already derived client-side — the server only verifies the old
    /// proof and persists the new material verbatim). On success the
    /// server persists the new credentials, bumps
    /// `tickets_invalid_before_ns`, and kills every other live session for
    /// this user (spec §12.5.3).
    ChangePasswordVerify {
        /// SCRAM proof recovered from the OLD password, bound to the
        /// pending challenge issued by [`DbRequest::ChangePasswordChallenge`].
        #[serde(with = "serde_bytes")]
        client_proof_old: Vec<u8>,
        /// New per-user salt (CSPRNG, client-generated).
        #[serde(with = "serde_bytes")]
        new_salt: Vec<u8>,
        /// New `stored_key = SHA256(HMAC(new_salted_pw, "Client Key"))`.
        #[serde(with = "serde_bytes")]
        new_stored_key: Vec<u8>,
        /// New `server_key = HMAC(new_salted_pw, "Server Key")`.
        #[serde(with = "serde_bytes")]
        new_server_key: Vec<u8>,
    },

    /// Grant or revoke superuser status on an existing SCRAM-directory
    /// account. Requires an already-superuser session AND an HMAC
    /// confirmation tag (same "did-you-mean-it" mechanism as destructive
    /// BatchOps, tasks #542/#551/#554 — see `check_destructive_hmacs`'s
    /// doc comment for the pattern this mirrors; this op is gated inline
    /// in its own handler rather than through that BatchOp-shaped
    /// function, since `SetSuperuser` is a top-level `DbRequest`, not a
    /// `BatchOp` inside a batch).
    ///
    /// NOT a `BatchOp` because `BatchOp`s dispatch through `shamir-db`'s
    /// engine, which has no handle to `shamir-server`'s real
    /// `FjallUserDirectory` (that bridge is task #559's `UserAdminPort`,
    /// not yet built). Mirrors `CreateScramUser`'s top-level shape.
    SetSuperuser {
        /// Target username.
        user: String,
        /// `true` to grant, `false` to revoke.
        on: bool,
        /// Hex-encoded HMAC-SHA256 tag over the canonical form — always
        /// required (unconditional, unlike `CreateFunctionOp`'s
        /// `security`/`secret_grants` fields).
        hmac: Option<String>,
    },

    /// Grant or revoke replication API access on an existing SCRAM-directory
    /// account (task #621 — mirrors SetSuperuser's shape/gate exactly, no
    /// last-remaining guard). Requires an already-superuser session AND an
    /// HMAC confirmation tag.
    SetReplicator {
        /// Target username.
        user: String,
        /// `true` to grant, `false` to revoke.
        on: bool,
        /// Hex-encoded HMAC-SHA256 tag over the canonical form — always
        /// required (unconditional, mirrors `SetSuperuser`'s gate).
        hmac: Option<String>,
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
    /// Successful [`DbRequest::SetSuperuser`] — the target's superuser flag
    /// is now `on`.
    SuperuserSet {
        /// Echoed target username.
        user: String,
        /// Echoed requested state (`true` = granted, `false` = revoked).
        on: bool,
    },
    /// Successful [`DbRequest::SetReplicator`] — the target's replicator
    /// flag is now `on`.
    ReplicatorSet {
        /// Echoed target username.
        user: String,
        /// Echoed requested state (`true` = granted, `false` = revoked).
        on: bool,
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

    /// Reply to [`DbRequest::ChangePasswordChallenge`] — the server-issued
    /// challenge view (spec §12.5 `challenge_cp`).
    ChangePasswordChallenge {
        /// Fresh CSPRNG nonce issued by the server for this challenge.
        #[serde(with = "serde_bytes")]
        server_nonce_cp: Vec<u8>,
        /// User's current salt (echoed for client convenience).
        #[serde(with = "serde_bytes")]
        salt: Vec<u8>,
        /// User's current KDF memory cost (KB) — `proof_old` uses these.
        kdf_memory_kb: u32,
        /// User's current KDF time cost (passes).
        kdf_time: u32,
        /// User's current KDF parallelism.
        kdf_parallelism: u32,
        /// User's current Argon2 algorithm version byte.
        kdf_argon2_version: u8,
    },
    /// Successful reply to [`DbRequest::ChangePasswordVerify`]. No payload
    /// beyond ok — the client already knows its own new credentials.
    ChangePasswordOk,
}
