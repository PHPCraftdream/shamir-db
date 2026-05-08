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

// --------------------------------------------------------------------------
// Wire schema
// --------------------------------------------------------------------------

/// Application-layer DB request (msgpack-encoded payload of
/// `RequestEnvelope.req`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DbRequest {
    /// Health check — no DB lookup.
    Ping,
    /// Execute a [`BatchRequest`] against the named database. The batch
    /// payload is forwarded verbatim to [`ShamirDb::execute`]; the full
    /// [`BatchResponse`] (records, stats, pagination, plan, transaction
    /// info) is returned to the client.
    Execute {
        /// Target database name (must already exist, or be created within
        /// the same batch via a `create_db` op).
        db: String,
        /// Batch payload — see `shamir_db::db::query::batch::BatchRequest`.
        batch: BatchRequest,
    },
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
    /// DB-layer failure (permission, planner, query, lock-timeout, …).
    /// Not a protocol error; the wire frame is a normal `ResponseEnvelope`.
    Error {
        /// Coarse classification so clients can switch without parsing
        /// the message. One of: `permission_denied`, `validation`,
        /// `limits`, `query`, `timeout`, `lock_timeout`, `unknown_db`.
        code: String,
        /// Human-readable detail.
        message: String,
    },
}

// --------------------------------------------------------------------------
// Handler
// --------------------------------------------------------------------------

/// Bridge handler — routes wire requests to a shared [`ShamirDb`] instance.
///
/// # Permissions (v1)
///
/// Admin / auth batch ops require `session.permissions.is_superuser`.
/// All other ops are accepted from any authenticated session. A future
/// patch will plumb per-role table-level RBAC by mapping into
/// `shamir_db::db::query::auth::SessionPermissions` and using
/// `execute_batch_with_permissions`.
#[derive(Clone)]
pub struct ShamirDbHandler {
    db: Arc<ShamirDb>,
}

impl ShamirDbHandler {
    /// Construct a handler over a shared [`ShamirDb`].
    pub fn new(db: Arc<ShamirDb>) -> Self {
        Self { db }
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
            DbRequest::Execute { db, batch } => self.execute(session, &db, batch),
        };

        rmp_serde::to_vec_named(&response).map_err(|e| format!("encode_error: {}", e))
    }
}

impl ShamirDbHandler {
    /// Run the admin gate, then forward to [`ShamirDb::execute`] on the
    /// current Tokio worker.
    fn execute(&self, session: &Session, db_name: &str, batch: BatchRequest) -> DbResponse {
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
