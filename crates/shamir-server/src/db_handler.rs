//! `RequestHandler` bridge — exposes the **full ShamirDb Batch API** over the
//! authenticated wire.
//!
//! After SCRAM auth, every request comes in as `RequestEnvelope { session_id,
//! request_id, req: Vec<u8> }`. The transport calls
//! `dispatch_request_view(view, store, lookup_tickets_invalid, handler)`,
//! which runs the §7.5 validity check and then invokes
//! [`RequestHandler::handle`]. This module implements that trait against a
//! shared [`ShamirDb`] instance and routes the payload straight into
//! [`ShamirDb::execute`] — i.e. into the canonical, fully-featured query
//! entry point of the database.
//!
//! # Wire schema (msgpack, `rmp_serde::to_vec_named` / `from_slice`)
//!
//! - [`DbRequest::Ping`] — health check (zero DB cost; useful for keepalive).
//! - [`DbRequest::Execute { db, batch }`] — wraps a complete
//!   [`BatchRequest`] (see `shamir_db::db::query::batch`). The batch is the
//!   single point of entry for every database operation:
//!     - reads (with WHERE / SELECT projections+aggregations / GROUP BY /
//!       ORDER BY / pagination / `count_total`),
//!     - writes (Insert / Update / Set (upsert) / Delete),
//!     - admin DDL (CreateDb / DropDb / CreateRepo / DropRepo / CreateTable /
//!       DropTable / CreateIndex / DropIndex / List),
//!     - auth ops (CreateUser / DropUser / CreateRole / DropRole / GrantRole /
//!       RevokeRole),
//!     - cross-query references via `{"$query": "@alias[].field"}`,
//!     - optional MVCC transactional semantics.
//!
//! [`DbResponse::Batch`] returns the **full** [`BatchResponse`] —
//! per-alias [`QueryResult`](shamir_db::db::query::read::QueryResult) with
//! `records: Vec<Value>`, [`QueryStats`](shamir_db::db::query::read::QueryStats),
//! [`PaginationInfo`](shamir_db::db::query::read::PaginationInfo), the
//! execution plan stages, total execution time, and transaction info. No
//! information is dropped or summarised by the bridge.
//!
//! # Permission gate (v1)
//!
//! The session-layer permission snapshot
//! ([`SessionPermissions`](shamir_connect::server::session::SessionPermissions))
//! currently tracks only `is_superuser` + `roles: Vec<String>`. This bridge
//! enforces a single coarse rule: **any [`BatchOp`] for which
//! [`BatchOp::is_admin`] returns true requires `is_superuser`**. Read/write
//! ops on data tables are accepted from any authenticated session.
//!
//! Fine-grained per-table RBAC (mapping role names → DB-side
//! [`SessionPermissions`](shamir_db::db::query::auth::SessionPermissions) +
//! [`execute_batch_with_permissions`]) is a follow-up item — the wire
//! schema does not need to change for it.
//!
//! # Error semantics
//!
//! `RequestHandler::handle` returns `Err(reason)` only for **protocol-level**
//! failures (msgpack decode, response encode). DB-layer failures
//! (admin denied, planner errors, query errors, lock timeouts) are returned
//! inside `Ok(bytes)` carrying a [`DbResponse::Error`] payload with a
//! coarse `kind` tag for clients to switch on without parsing prose.
//!
//! # Async bridge
//!
//! `RequestHandler::handle` is sync; [`ShamirDb::execute`] is async. We
//! bridge with `tokio::task::block_in_place` + `Handle::current().block_on`
//! so the future is driven on the current Tokio worker without spawning
//! a second runtime. This **requires** a multi-thread Tokio runtime — the
//! integration tests use `#[tokio::test(flavor = "multi_thread")]` and
//! the production server starts the multi-thread runtime in `main.rs`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::Session;

use shamir_db::db::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_db::db::ShamirDb;

use shamir_connect::common::crypto::random_array;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

use crate::user_directory::RedbUserDirectory;
use crate::version::{check_query_lang, CURRENT_QUERY_LANG_VERSION};

// --------------------------------------------------------------------------
// Wire schema
// --------------------------------------------------------------------------

/// Application-layer DB request (msgpack-encoded payload of
/// `RequestEnvelope.req`).
///
/// # Versioning
///
/// `Execute` carries an explicit `query_version: u32` so the server can
/// reject unknown versions before invoking the DB layer. The supported
/// list is hardcoded in [`crate::version::SUPPORTED_QUERY_LANG_VERSIONS`].
/// Today there is exactly one supported version (1); when the schema of
/// `BatchRequest` changes incompatibly, bump the version, add it to the
/// supported list, and either translate or reject older versions
/// explicitly.
///
/// `Ping` does not carry a version — it has no payload to interpret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DbRequest {
    /// Health check — no DB lookup, no version negotiation.
    Ping,
    /// Execute a [`BatchRequest`] against the named database. The batch
    /// payload is forwarded verbatim to [`ShamirDb::execute`]; the full
    /// [`BatchResponse`] (records, stats, pagination, plan, transaction
    /// info) is returned to the client.
    Execute {
        /// Query-language version. Hardcoded supported list lives in
        /// [`crate::version::SUPPORTED_QUERY_LANG_VERSIONS`]. Today: `1`.
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name (must already exist, or be created within
        /// the same batch via a `create_db` op).
        db: String,
        /// Batch payload — see `shamir_db::db::query::batch::BatchRequest`.
        batch: BatchRequest,
    },
    /// Create a SCRAM-authenticatable user — the kind of user that can
    /// actually log in over the wire. Distinct from `BatchOp::CreateUser`
    /// (which creates a DB-level user for table-level permissions). The
    /// server runs Argon2id with its configured KDF defaults to derive the
    /// `stored_key` / `server_key` for the supplied password, then writes
    /// the record to the durable [`RedbUserDirectory`].
    ///
    /// Requires `session.permissions.is_superuser`.
    CreateScramUser {
        /// Username (will be NFC + UsernameCaseMapped normalised by the
        /// `RedbUserDirectory` write path).
        name: String,
        /// Plaintext password. Hashed server-side; the server zeroizes the
        /// buffer immediately after derivation.
        password: String,
        /// Roles to grant. Use `["superuser"]` to give admin powers; any
        /// other role string is opaque to the protocol layer (RBAC
        /// matching is application-defined).
        #[serde(default)]
        roles: Vec<String>,
    },
}

/// Default `query_version` used when the field is absent from an older
/// client's payload. Today: `1` (the only supported version), so an
/// older client without the field is treated as a v1 request rather
/// than a hard error.
fn default_query_version() -> u32 {
    CURRENT_QUERY_LANG_VERSION
}

/// Application-layer DB response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DbResponse {
    /// Reply to [`DbRequest::Ping`].
    Pong,
    /// Successful batch execution. Carries the full [`BatchResponse`] with
    /// no fields dropped.
    Batch {
        /// Echoed `id`, results map, execution_plan, execution_time_us,
        /// optional transaction info.
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
        /// Coarse classification so clients can switch without parsing
        /// the message. One of: `permission_denied`, `validation`,
        /// `limits`, `query`, `timeout`, `lock_timeout`, `unknown_db`,
        /// `not_supported`, `user_exists`.
        code: String,
        /// Human-readable detail.
        message: String,
    },
}

// --------------------------------------------------------------------------
// Handler
// --------------------------------------------------------------------------

/// Optional admin glue — supplied by the boot path so admin ops that
/// require server-side state (the SCRAM user directory + KDF cost
/// parameters) can run. Tests that don't need user creation omit this.
#[derive(Clone)]
pub struct AdminGlue {
    /// Directory that stores SCRAM-authenticatable users.
    pub user_dir: Arc<RedbUserDirectory>,
    /// KDF defaults applied to newly created users so they can log in
    /// against the same listener policy.
    pub kdf: KdfParams,
}

/// Bridge handler — routes wire requests to a shared [`ShamirDb`] instance.
///
/// # Permissions (v1)
///
/// Admin / auth batch ops AND [`DbRequest::CreateScramUser`] require
/// `session.permissions.is_superuser`. All other ops are accepted from any
/// authenticated session. A future patch will plumb per-role table-level
/// RBAC by mapping into
/// `shamir_db::db::query::auth::SessionPermissions` and using
/// `execute_batch_with_permissions`.
#[derive(Clone)]
pub struct ShamirDbHandler {
    db: Arc<ShamirDb>,
    /// `None` means the handler was constructed without admin support;
    /// `CreateScramUser` requests will return `not_supported`.
    admin: Option<AdminGlue>,
}

impl ShamirDbHandler {
    /// Construct a handler over a shared [`ShamirDb`] without admin support.
    /// Use [`Self::with_admin`] when SCRAM user creation should be possible.
    pub fn new(db: Arc<ShamirDb>) -> Self {
        Self { db, admin: None }
    }

    /// Construct a handler with admin (SCRAM user-creation) support.
    pub fn with_admin(db: Arc<ShamirDb>, admin: AdminGlue) -> Self {
        Self { db, admin: Some(admin) }
    }

    /// Reference to the underlying [`ShamirDb`] (for tests / admin glue).
    pub fn db(&self) -> &Arc<ShamirDb> {
        &self.db
    }
}

impl RequestHandler for ShamirDbHandler {
    fn handle(&self, session: &Session, req: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let request: DbRequest = rmp_serde::from_slice(req)
            .map_err(|e| format!("invalid_request: {}", e))?;

        let response = match request {
            DbRequest::Ping => DbResponse::Pong,
            DbRequest::Execute { query_version, db, batch } => {
                self.execute(session, query_version, &db, batch)
            }
            DbRequest::CreateScramUser { name, password, roles } => {
                self.create_scram_user(session, name, password, roles)
            }
        };

        rmp_serde::to_vec_named(&response).map_err(|e| format!("encode_error: {}", e))
    }
}

impl ShamirDbHandler {
    /// Run the version check + admin gate, then forward to
    /// [`ShamirDb::execute`] on the current Tokio worker.
    fn execute(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        batch: BatchRequest,
    ) -> DbResponse {
        // Query-language version dispatch — fast reject before any DB work.
        if let Err(e) = check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }

        // Admin / auth gate.
        if !session.permissions.read().is_superuser {
            for (alias, entry) in &batch.queries {
                if entry.op.is_admin() {
                    return DbResponse::Error {
                        code: "permission_denied".into(),
                        message: format!(
                            "query '{}' requires superuser (admin/auth op)",
                            alias
                        ),
                    };
                }
            }
        }

        match run_blocking(self.db.execute(db_name, &batch)) {
            Ok(response) => DbResponse::Batch { response },
            Err(e) => DbResponse::Error {
                code: error_code(&e).to_string(),
                message: e.to_string(),
            },
        }
    }

    /// Create a SCRAM-authenticatable user. Server-side Argon2id is run
    /// inside `block_in_place` to keep the Tokio worker responsive.
    fn create_scram_user(
        &self,
        session: &Session,
        name: String,
        password: String,
        roles: Vec<String>,
    ) -> DbResponse {
        if !session.permissions.read().is_superuser {
            return DbResponse::Error {
                code: "permission_denied".into(),
                message: "create_scram_user requires superuser".into(),
            };
        }
        let admin = match &self.admin {
            Some(a) => a,
            None => {
                return DbResponse::Error {
                    code: "not_supported".into(),
                    message: "handler built without AdminGlue (no user_dir)".into(),
                }
            }
        };

        // Move password into a zeroizing buffer right away. `Zeroizing`
        // wipes on Drop, so we don't need an explicit `.zeroize()` call —
        // both the success and error paths drop `pw_buf` before returning.
        let pw_buf: Zeroizing<Vec<u8>> = Zeroizing::new(password.into_bytes());
        let salt: [u8; 16] = random_array();

        // Argon2id is CPU-heavy — wrap in block_in_place so we don't stall
        // the runtime worker.
        let derived = match tokio::task::block_in_place(|| {
            DerivedKeys::derive(&pw_buf, &salt, &admin.kdf)
        }) {
            Ok(d) => d,
            Err(e) => {
                return DbResponse::Error {
                    code: "query".into(),
                    message: format!("argon2id: {e}"),
                };
            }
        };
        drop(pw_buf);

        let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
        server_key_z.copy_from_slice(&derived.server_key[..]);
        let record = UserRecord {
            salt,
            stored_key: derived.stored_key,
            server_key: server_key_z,
            kdf_params: admin.kdf,
            tickets_invalid_before_ns: 0,
        };

        let user_id = match admin.user_dir.insert(name.clone(), record) {
            Ok(id) => id,
            Err(e) => {
                let msg = e.to_string();
                let code = if msg.contains("exists") { "user_exists" } else { "query" };
                return DbResponse::Error {
                    code: code.into(),
                    message: msg,
                };
            }
        };
        if !roles.is_empty() {
            // Best-effort role attach. now_ns=0 means "don't bump session
            // validity epoch" — no existing sessions for a brand-new user.
            let _ = admin.user_dir.update_roles(&name, roles, 0);
        }

        DbResponse::UserCreated {
            name,
            user_id: user_id.to_vec(),
        }
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/// Coarse classification of a [`BatchError`] for the wire `code` tag.
fn error_code(e: &BatchError) -> &'static str {
    match e {
        BatchError::TooManyQueries { .. } | BatchError::TooDeep { .. } => "limits",
        BatchError::CircularDependency { .. } | BatchError::UnknownAlias { .. } => "validation",
        BatchError::Timeout { .. } => "timeout",
        BatchError::LockTimeout { .. } => "lock_timeout",
        BatchError::QueryError { alias, message } => {
            // ShamirDb::execute maps "Database not found" through QueryError
            // with empty alias — surface that distinctly so clients can
            // tell wrong-db from wrong-query.
            if alias.is_empty() && message.contains("not found") {
                "unknown_db"
            } else {
                "query"
            }
        }
    }
}

/// Bridge an async future to a sync caller running inside a Tokio worker.
///
/// `block_in_place` lets us call `block_on` without panicking with
/// "Cannot start a runtime from within a runtime". Requires a multi-thread
/// runtime — single-thread (`current_thread`) flavor would also panic.
fn run_blocking<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}
