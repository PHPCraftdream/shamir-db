//! `DbRequest` / `DbResponse` — the application-layer payload of
//! `RequestEnvelope.req` / `ResponseEnvelope.res` after the SCRAM
//! handshake completes.

use serde::{Deserialize, Serialize};

use crate::batch::options::deserialize_optional_field;

use crate::admin::{ActivityPage, CancelOutcome};

use crate::batch::{
    BatchRequest, BatchResponse, IsolationLevel, TransactionInfo, VisibilityState, VisibilityWait,
};
use crate::durability::DurabilityOverride;
use crate::read::QueryRecord;
use crate::read::{QueryResult, ReadQuery};
use crate::wire::change_password_secrets::ChangePasswordSecrets;
use crate::wire::cursor_id::CursorId;
use crate::wire::repl::{ReplRequest, ReplResponse};
use crate::wire::token_admin::{ApiTokenChannels, ApiTokenMode, ApiTokenView};
use shamir_types::types::record_id::RecordId;

/// Current query-language version. Bumped when the on-the-wire
/// `BatchRequest` schema changes incompatibly. The server keeps a
/// hardcoded supported list (`SUPPORTED_QUERY_LANG_VERSIONS`) and
/// rejects unknown versions before invoking the DB layer.
///
/// v2: server now supports MessagePack id-keyed write/read pass-through.
/// Advertised to the client via `WireAuthOk.server_query_version` /
/// `WireResumeOkWire.server_query_version`; clients opt in only when
/// they see `server_query_version >= 2`.
/// v3: committed outcomes require explicit visibility metadata and opt-in.
/// v4: request options are typed (`durability: buffered|synced` enum, NEW
/// `index_visibility: sync|async` replacing the old
/// `durability: "async_index"` string, `isolation` strict enum); unknown
/// request-tree keys are rejected, except compatibility-preserved
/// shared storage/response types and open user-data maps; v3 and older are
/// rejected by the server.
pub const CURRENT_QUERY_LANG_VERSION: u32 = 4;

fn deserialize_tx_isolation<'de, D>(deserializer: D) -> Result<Option<IsolationLevel>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_field(deserializer, "isolation")
}
fn default_query_version() -> u32 {
    // Missing version is legacy, never an implicit opt-in to v3.
    0
}

/// Application-layer DB request (msgpack-encoded payload of
/// `RequestEnvelope.req`).
///
/// `ChangePasswordVerify`'s secret material lives in
/// [`ChangePasswordSecrets`], whose manual `Debug` redacts every byte
/// field — so the derived `Debug` here is safe for logging on every
/// variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // dispatch enum: rarely on the stack; mirrors BatchOp::Read(ReadQuery)
pub enum DbRequest {
    /// Health check — no DB lookup, no version negotiation.
    Ping,
    /// Read-only settlement observation; positions belong to this repository.
    WaitForVisibility {
        #[serde(default = "default_query_version")]
        query_version: u32,
        db: String,
        repo: String,
        version: u64,
        visibility_wait: VisibilityWait,
    },
    /// Execute a [`BatchRequest`] against the named database. The batch
    /// payload is forwarded verbatim to `ShamirDb::execute`; the full
    /// [`BatchResponse`] (records, stats, pagination, plan, transaction
    /// info) is returned to the client.
    Execute {
        /// Query-language version. Omission is legacy version 0.
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
        /// Query-language version. Omission is legacy version 0.
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name.
        db: String,
        /// The single repo the transaction is scoped to.
        repo: String,
        /// Isolation level (typed enum, v4) — same vocabulary as
        /// [`crate::batch::BatchRequest::isolation`].
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_tx_isolation"
        )]
        isolation: Option<IsolationLevel>,
    },
    /// Run a batch inside an already-open interactive transaction. The tx is
    /// NOT committed — state accumulates in the parked transaction. The
    /// batch's `transactional` flag is ignored here (the handle already
    /// establishes tx mode).
    TxExecute {
        /// Query-language version. Omission is legacy version 0.
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
        /// Explicit v3 opt-in; omission is rejected as a legacy request.
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Commit acknowledgement override (contract only in VIS-1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visibility_wait: Option<VisibilityWait>,
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
    ChangePasswordVerify(ChangePasswordSecrets),

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

    /// Create a scoped API token for the caller or an authorized user.
    CreateApiToken {
        /// Human-readable label for the token.
        label: String,
        /// Optional lifetime in seconds; absence selects the configured default.
        #[serde(default)]
        ttl_secs: Option<u64>,
        /// Scope mode (`read_only` or `read_write`).
        mode: Option<ApiTokenMode>,
        /// Allowed channels (`rest`, `ws`, or `both`).
        channels: Option<ApiTokenChannels>,
        /// Optional database allowlist; `None` permits all accessible databases.
        databases: Option<Vec<String>>,
        /// Optional target username, as resolved by the server directory.
        for_user: Option<String>,
        /// Optional confirmation HMAC.
        hmac: Option<String>,
    },
    /// List API tokens for the caller or an authorized user.
    ListApiTokens {
        /// Optional target username, as resolved by the server directory.
        for_user: Option<String>,
        /// Optional confirmation HMAC.
        hmac: Option<String>,
    },
    /// Revoke an API token identified by base64url-encoded 16-byte ID.
    RevokeApiToken {
        /// Token ID encoded as base64url.
        id: String,
        /// Optional confirmation HMAC.
        hmac: Option<String>,
    },
    /// Revoke all API tokens for the caller or a specified user.
    RevokeUserTokens {
        /// Optional target username, resolved by the directory.
        user: Option<String>,
        /// Optional confirmation HMAC.
        hmac: Option<String>,
    },

    // --- FG-5a: server-side result cursors (wire protocol only; the
    // engine/session state that actually backs these lands in FG-5b — see
    // `docs/guide-docs/client-server-protocol-spec/CURSORS.md`) ---
    /// Open a server-side cursor over a read query and return its first
    /// page. The server mints an opaque [`CursorId`] and parks cursor state
    /// (an MVCC as_of snapshot + scan position) bound to the session —
    /// FG-5b. `page_size` bounds how many records come back per page
    /// (this call's page AND every subsequent `FetchNext`'s default,
    /// though `FetchNext` may request a different size per call).
    CreateCursor {
        /// Query-language version. Omission is legacy version 0.
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name.
        db: String,
        /// The read query the cursor scans — same [`ReadQuery`] shape a
        /// batch `Read` op uses.
        query: ReadQuery,
        /// Maximum records returned per page (this call's page, and the
        /// default for subsequent `FetchNext` calls that omit an override).
        page_size: u32,
    },
    /// Fetch the next page from an already-open cursor.
    ///
    /// `page_size` may differ from the value used at `CreateCursor` time or
    /// any prior `FetchNext` — client-controlled per-call backpressure.
    FetchNext {
        /// The cursor minted by [`DbRequest::CreateCursor`].
        cursor_id: CursorId,
        /// `Some(n)` requests `n` records for THIS page (client-controlled
        /// per-call backpressure, same as before). `None` (an omitted field
        /// on the wire — msgpack's normal "missing key" semantics for an
        /// `Option`) falls back to the cursor's stored `CreateCursor`-time
        /// default (`Cursor::default_page_size`).
        #[serde(default)]
        page_size: Option<u32>,
    },
    /// Explicitly close (cancel) an open cursor, releasing its server-side
    /// state early (idle-timeout eviction is the implicit counterpart,
    /// FG-5b).
    ///
    /// Idempotent: canceling an already-closed or unknown cursor id is NOT
    /// an error — the server replies with
    /// [`DbResponse::CursorClosed`] either way.
    CancelCursor {
        /// The cursor to close.
        cursor_id: CursorId,
    },
    /// Poll the status of a DDL operation by its operation ID.
    ///
    /// Returns the current `DdlOpStatus` for the given `op_id`, or `None`
    /// if the operation is unknown (GC'd, never existed, or a pre-RFC op).
    /// This is the mechanism that allows a client to learn whether a
    /// crashed-and-recovered DDL op actually completed.
    GetDdlOpStatus {
        /// Target database name.
        db: String,
        /// Target repository name.
        repo: String,
        /// Target table name (the table on which the DDL op ran).
        table: String,
        /// The operation ID to query (from `QueryResult::op_id`).
        op_id: String,
    },

    /// Read-only operator status observation (admin-shell PR3, P1).
    ///
    /// Carries no parameters: the reply ([`DbResponse::Status`])
    /// describes the whole server. The observed format epoch and stamp
    /// outcome were captured BY THE LAUNCHER AT BOOT via the on-disk
    /// stamp `inspect` while the boot lock was held — never re-read
    /// from disk at request time. Requires a superuser-class session;
    /// a token session never reaches it (the token perimeter denies
    /// it with `token_request_denied`).
    Status,

    /// Typed exact-key point read (admin-shell PR3, P2). NOT a
    /// `BatchOp`: no cursor is created and no cursor path is engaged.
    /// The server applies `Database` `Read` THEN `Table` `Read` ACLs
    /// (ancestor check before any lookup), the same repository
    /// quarantine as the batch path, a typed-key size cap and the
    /// existing result-size / in-flight response controls. `key` is a
    /// `RecordId` (fixed 16-byte msgpack `bin` on the wire); a
    /// malformed key or a missing record is a typed refusal, not a
    /// panic and not a scan. Replies [`DbResponse::Record`].
    GetRecord {
        /// Target database name.
        db: String,
        /// Target repository name.
        repo: String,
        /// Target table name.
        table: String,
        /// Exact record key (base58-decoded 16-byte id, fixed
        /// 16-byte msgpack `bin` on the wire — see the
        /// `record_id_wire` codec module below).
        #[serde(with = "record_id_wire")]
        key: RecordId,
    },
}

/// Fixed 16-byte msgpack `bin` codec for [`RecordId`] on the wire.
///
/// Deliberately does NOT use `RecordId`'s own `Deserialize`: that impl
/// binds `&[u8]` (borrowed), which fails when the msgpack decoder
/// buffers content. Instead it uses `serde_bytes::ByteArray<16>`, whose
/// visitor accepts both borrowed bytes and owned `byte_buf` (and whose
/// `visit_seq` path is bounded by the fixed size — no unbounded
/// allocation). Serialization emits the 16 bytes as msgpack `bin`;
/// deserialization requires EXACTLY 16 bytes and is a typed decode
/// error otherwise.
///
/// Private on purpose: the `serde(with = "record_id_wire")` attribute
/// resolves the path inside this module, so no external caller can ever
/// need it (admin-shell PR3 review: minimal visibility).
mod record_id_wire {
    use super::RecordId;
    use serde_bytes::{Deserialize, Serialize};

    /// Serialize the id as msgpack `bin` (16 bytes).
    pub(super) fn serialize<S: serde::Serializer>(id: &RecordId, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::ByteArray::new(id.0).serialize(s)
    }

    /// Deserialize from msgpack `bin`, requiring EXACTLY 16 bytes.
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<RecordId, D::Error> {
        let arr = serde_bytes::ByteArray::<16>::deserialize(d)?;
        Ok(RecordId(arr.into_array()))
    }
}

/// Application-layer DB response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DbResponse {
    /// Reply to [`DbRequest::Ping`].
    Pong,
    /// Settlement through a position, not evidence an unknown write committed.
    VisibilityObserved {
        version: u64,
        visibility: VisibilityState,
    },
    /// Structured read-wait failure; no older snapshot was substituted.
    AsOfVisibilityTimeout {
        code: String,
        message: String,
        requested: u64,
        visible_through: u64,
        blocking_version: u64,
    },
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
    /// Successful API-token creation; secret appears only in this response.
    ApiTokenCreated(crate::auth::token_admin::CreatedApiToken),
    /// API-token listing response, containing public metadata rows only.
    ApiTokensListed {
        /// Public API-token metadata rows.
        tokens: Vec<ApiTokenView>,
    },
    /// Confirmation that one token was revoked.
    ApiTokenRevoked,
    /// Number of tokens revoked for the selected user.
    ApiUserTokensRevoked {
        /// Number of tokens revoked.
        count: u64,
    },
    /// Successful [`crate::batch::BatchOp::ListActivity`] — one bounded page
    /// of the operator activity view (R4b step 1). Rows carry ids, actor,
    /// db, op-kind names, timing and state only; never credentials or
    /// literals (D3).
    ActivityPage {
        /// The rendered rows plus the opaque continuation token, if more
        /// remain.
        page: ActivityPage,
    },

    /// DB-layer failure (permission, planner, query, lock-timeout, …).
    /// Not a protocol error; the wire frame is a normal `ResponseEnvelope`.
    Error {
        /// Coarse classification of the failure. The full vocabulary actually
        /// emitted by the server (see `shamir-server` `db_handler`):
        ///
        /// - Auth / routing: `permission_denied`, `access_denied`,
        ///   `read_only_replica`, `bad_role`, `unsupported_query_version`.
        /// - Wire / decode-level rejection: `invalid_request` — malformed
        ///   msgpack, unknown `BatchRequest` key (`deny_unknown_fields`), or
        ///   bad enum value (e.g. `durability: "syncedd"`); the offending
        ///   field is named in the message.
        /// - Destructive-op HMAC gate: `hmac_required`, `hmac_mismatch`.
        /// - API-token administration: `token_label_exists`, `token_limit_reached`,
        ///   `token_ttl_invalid`, `token_not_found`, `token_browser_session`,
        ///   `token_request_denied`, `token_self_service_disabled`,
        ///   `session_invalidated`, `invalid_username`, `not_found`, `internal`.
        /// - User / auth administration (Argon2 permit gate): `server_busy` —
        ///   an Argon2 KDF permit could not be acquired before the
        ///   administrative write (e.g. SCRAM user creation); the refused
        ///   creation itself made no change, so retry after backoff (this is
        ///   not a promise that other operations in a surrounding batch did
        ///   not mutate state).
        /// - Batch / planner: `limits`, `validation`, `query`, `timeout`,
        ///   `lock_timeout`, `nesting_too_deep`, `unknown_db`, `not_supported`,
        ///   `user_exists`, `cancelled`.
        /// - Interactive transactions: `tx_conflict`, `tx_not_found`,
        ///   `tx_already_open`, `tx_in_tx`, `tx_forbidden`, `tx_write`,
        ///   `tx_wrong_db`, `tx_wrong_repo`, `tx_cross_repo_not_supported`,
        ///   `tx_too_large`, `tx_expired`, `tx_error`.
        /// - Commit admission (via `QueryError` structured `code`):
        ///   `wal_requires_reopen`, `wal_retention_exceeded` (transient —
        ///   the refusal clears once the drainer truncates the retained
        ///   sealed WAL back under `durability.wal_retention_max_bytes`).
        /// - Foreign keys: `fk_violation`, `fk_restrict`, `fk_cascade_depth`,
        ///   `fk_requires_index`, `fk_actions`, `fk_on_update`,
        ///   `fk_update_unsupported_new_value`.
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

    // --- FG-5a: server-side result cursors (wire protocol only — see
    // `docs/guide-docs/client-server-protocol-spec/CURSORS.md`) ---
    /// Reply to both [`DbRequest::CreateCursor`] (first page) and
    /// [`DbRequest::FetchNext`] (subsequent pages).
    CursorPage {
        /// The cursor this page belongs to (echoed on `CreateCursor`, the
        /// same id the caller supplied on `FetchNext`).
        cursor_id: CursorId,
        /// This page's records/stats — reuses the existing [`QueryResult`]
        /// shape, so a cursor page looks exactly like a regular read result.
        page: QueryResult,
        /// `true` if more pages remain (a further `FetchNext` will return
        /// at least one more record); `false` when this page was the last
        /// and the server has already released the cursor.
        has_more: bool,
    },
    /// Reply to [`DbRequest::CancelCursor`] — also the (idempotent) reply
    /// when the target cursor was already closed or never existed.
    CursorClosed {
        /// The cursor id that is now (or already was) closed.
        cursor_id: CursorId,
    },
    /// Reply to [`DbRequest::GetDdlOpStatus`].
    ///
    /// Returns the full operation status if found, `None` if the operation
    /// is unknown (GC'd, never existed, or a pre-RFC op that had no op_id).
    DdlOpStatus {
        /// The operation's full status, or `None` if unknown.
        status: Option<crate::read::DdlOpStatus>,
    },
    /// Reply to [`crate::batch::BatchOp::CancelRequest`].
    CancelOutcome {
        /// The cancellation attempt's result, not the target batch's reply.
        outcome: CancelOutcome,
    },
    /// Reply to [`DbRequest::Status`] — the operator state snapshot
    /// ([`OperatorStatus`]).
    Status {
        /// The observed server state.
        status: OperatorStatus,
    },
    /// Reply to [`DbRequest::GetRecord`] — the one record the exact
    /// key resolved to (the same shape a batch read returns). A
    /// missing record is a typed `DbResponse::Error` with code
    /// `not_found`, never an empty success.
    Record {
        /// The record at the requested key.
        record: QueryRecord,
    },
}

/// Boot-time format-stamp observation, mirrored from the
/// replication owner's `StampOutcome` (six variants, names kept in
/// sync by hand — the owner's enum is not re-exported across the
/// layer boundary). Captured BY THE LAUNCHER AT BOOT via the on-disk
/// stamp `inspect` while the boot lock was held; NOT the bare
/// getter's outcome-only value and NOT a process constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusStampOutcome {
    /// Stamp written into an empty directory.
    Created,
    /// Stampless data adopted at the base epoch.
    Adopted,
    /// Valid stamp, current epoch, nothing rewritten.
    Unchanged,
    /// `last_opened_by` rewritten (binary version changed).
    VersionUpdated,
    /// Inspect on a stampless directory.
    InspectedStampless,
    /// Inspect on an older-epoch directory.
    InspectedOlder,
}

/// Replication role, derived from the node's configured mode at
/// launch. `Follower` means this node follows a leader; `Unknown`
/// covers any configuration the launcher could not map (including
/// replication disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusRole {
    /// This node follows a leader.
    Follower,
    /// Role could not be determined (no configured mode / unknown
    /// configuration).
    Unknown,
}

/// One replication observation from the exported snapshot.
///
/// `gap_versions_merged` is a MERGED, NON-AUTHORITATIVE aggregate:
/// the exporter cannot distinguish per-source gaps, and no
/// timestamp plumbing exists, so `age` is ALWAYS `None` (`unknown`)
/// — consumers must never present it as lag against a specific
/// leader. `receipt_age_ns` is the age of the last durable
/// observation receipt when one is available — a durable write
/// observation, not necessarily a pull.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusReplication {
    /// Database name.
    pub db: String,
    /// Repository name.
    pub repo: String,
    /// Merged non-authoritative gap aggregate per repository (see
    /// the type docs for the caveat).
    pub gap_versions_merged: u64,
    /// Always `None` (`unknown`): no timestamp plumbing exists.
    pub age: Option<u64>,
    /// Nanoseconds since the last durable observation receipt, when
    /// available. This is the receipt age of a durable observation, not
    /// necessarily the age of a pull.
    pub receipt_age_ns: Option<u64>,
}

/// WAL watermark / pending / commit telemetry for one repository,
/// with the semantics of `durability_export.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusWal {
    /// `true` when the WAL backend is the in-memory backend.
    pub backend_mem: bool,
    /// Current WAL watermark.
    pub watermark_w: u64,
    /// Highest version recoverable from the WAL after a crash.
    pub recoverable_through: u64,
    /// Span between the current watermark and the highest version
    /// recoverable from the WAL (`watermark_w - recoverable_through`) —
    /// a bound, NOT the actual number of unflushed records.
    pub pending_versions: u64,
    /// First pending version, when any exist.
    pub first_pending_version: Option<u64>,
    /// Age in nanoseconds of the first pending version, when known.
    pub first_pending_age_ns: Option<u64>,
    /// Number of `sync_now` calls.
    pub sync_now_calls: u64,
    /// Number of sync rounds run.
    pub sync_rounds: u64,
    /// Number of syncs coalesced away.
    pub sync_coalesced: u64,
    /// Number of sync errors.
    pub sync_errors: u64,
    /// Total sync duration in nanoseconds.
    pub sync_duration_ns: u64,
}

/// Effective vs stored durability for one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusRepository {
    /// Database name.
    pub db: String,
    /// Repository name.
    pub repo: String,
    /// The durability override actually in force.
    pub effective: DurabilityOverride,
    /// The override as stored (may differ from `effective`); `None`
    /// when nothing is stored.
    pub stored: Option<DurabilityOverride>,
    /// WAL telemetry for this repository.
    pub wal: StatusWal,
}

/// Operator state snapshot ([`DbResponse::Status`]). Built from the
/// launcher's boot capture (server version, on-disk format epoch, boot
/// stamp outcome, configured node id, configured role), the replication
/// owner's process-global observations, each open repository's live
/// durability / WAL telemetry, and ONE stored durability-override
/// catalogue read of the RUNNING system store — that catalogue read
/// happens at REQUEST time, so `status` (and `version show`, which reads
/// the same snapshot) can fail with a typed code when it fails; only the
/// boot stamp is never reread.
///
/// The reply is capped like the batch path: the encoded `DbResponse` is
/// measured once and refused whole with `result_too_large` when it
/// exceeds the server's result-size limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OperatorStatus {
    /// Server binary version string.
    pub server_version: String,
    /// On-disk format epoch observed by the launcher at boot.
    pub format_epoch: u64,
    /// Boot-time stamp outcome observed by the launcher at boot.
    pub stamp_outcome: StatusStampOutcome,
    /// Monotonic uptime in milliseconds.
    pub uptime_ms: u64,
    /// Replication role from the node's configured mode at launch.
    pub role: StatusRole,
    /// Stable node id, when one is configured.
    pub node_id: Option<String>,
    /// Per-repository replication observations (merged, non-
    /// authoritative — see [`StatusReplication`]).
    pub replication: Vec<StatusReplication>,
    /// Per-repository durability / WAL state.
    pub repositories: Vec<StatusRepository>,
}

/// Stable codes used by [`DbResponse::Error`].
pub mod error_code {
    /// The target batch stopped cooperatively after a cancellation request.
    pub const CANCELLED: &str = "cancelled";
}
