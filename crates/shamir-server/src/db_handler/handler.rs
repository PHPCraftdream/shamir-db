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
//!   [`BatchRequest`] (see `shamir_db::query::batch`). The batch is the
//!   single point of entry for every database operation:
//!     - reads (with WHERE / SELECT projections+aggregations / GROUP BY /
//!       ORDER BY / pagination / `count_total`),
//!     - writes (Insert / Update / Set (upsert) / Delete),
//!     - admin DDL (CreateDb / DropDb / CreateRepo / DropRepo / CreateTable /
//!       DropTable / CreateIndex / DropIndex / List),
//!     - auth ops (CreateUser / DropUser / GrantRole / RevokeRole),
//!     - cross-query references via `{"$query": "@alias[].field"}`,
//!     - optional MVCC transactional semantics.
//!
//! [`DbResponse::Batch`] returns the **full** [`BatchResponse`] —
//! per-alias [`QueryResult`](shamir_db::query::read::QueryResult) with
//! `records: Vec<Value>`, [`QueryStats`](shamir_db::query::read::QueryStats),
//! [`PaginationInfo`](shamir_db::query::read::PaginationInfo), the
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
//! [`SessionPermissions`](shamir_db::query::auth::SessionPermissions) +
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
//! # Async model
//!
//! `RequestHandler::handle` is now async (returns a boxed future). The
//! `connection::request_loop` awaits it directly — no blocking-pool bridge
//! (`spawn_blocking` / `run_blocking`) is needed on the dispatch path.
//! Tokio worker threads are never parked; every `.await` inside the handler
//! yields the worker back to the scheduler. CPU-bound work (Argon2id key
//! derivation) is delegated to `tokio::task::spawn_blocking` so the worker
//! remains free during derivation.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionStore};

use shamir_db::access::Actor;
use shamir_db::query::batch::{BatchError, BatchOp, BatchRequest};
use shamir_db::ShamirDb;

// Wire DTOs are defined once in `shamir-query-types::wire` so that the
// SDK (`shamir-client`) and the server share the same definition. Re-
// exported here so existing `shamir_server::db_handler::DbRequest` import
// paths keep resolving.
pub use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::tx_registry::TxRegistry;

use super::admin::{
    change_password_challenge, change_password_verify, check_destructive_hmacs, create_scram_user,
    is_coarse_admin_gate_exempt, set_replicator, set_superuser, AdminGlue,
};
use super::config::{NodeMode, QueryLimitsCap, SlowQueryConfig, TxLimitsCap};
use super::subscribe_handler;

/// Absolute lifetime cap for an interactive (Phase B) transaction — bounds
/// how long any one open tx can pin MVCC GC, even if a client keeps it busy.
/// Mirrors the engine's `DEFAULT_MAX_TX_LIFETIME` (5 min); the commit path's
/// own `is_expired` check is the final backstop.
pub(super) const INTERACTIVE_TX_MAX_LIFETIME: Duration = Duration::from_secs(300);

// --------------------------------------------------------------------------
// Wire schema
// --------------------------------------------------------------------------
//
// `DbRequest` / `DbResponse` are defined in `shamir-query-types::wire`
// (re-exported above). Server-side validation of `query_version`
// against the hardcoded `SUPPORTED_QUERY_LANG_VERSIONS` list happens
// inside `RequestHandler::handle` below.

// --------------------------------------------------------------------------
// Handler
// --------------------------------------------------------------------------

/// Resolve the [`Actor`] for the current session.
///
/// Superuser sessions get `Actor::Admin(principal64(session.user_id))` —
/// bypasses the Shomer gate exactly like `Actor::System`, but attributes
/// ownership of admin-created resources to the real account instead of
/// collapsing to `owner = 0`. Regular sessions get
/// `Actor::User(principal64(session.user_id))`. `user_id` is the directory-
/// minted 16-byte id stamped on the session at login — NOT derived from the
/// username — so a dropped-and-recreated account gets a fresh id even if it
/// reuses the same name (closes the identity-inheritance-on-recreate bug,
/// design doc §1.1 finding 3).
pub(super) fn session_actor(session: &Session) -> Actor {
    let id = shamir_types::access::principal64(session.user_id);
    if session.permissions.is_superuser {
        Actor::Admin(id)
    } else {
        Actor::User(id)
    }
}

/// Bridge handler — routes wire requests to a shared [`ShamirDb`] instance.
///
/// # Permissions (v1)
///
/// Admin / auth batch ops AND [`DbRequest::CreateScramUser`] require
/// `session.permissions.is_superuser`. All other ops are accepted from any
/// authenticated session. A future patch will plumb per-role table-level
/// RBAC by mapping into
/// `shamir_db::query::auth::SessionPermissions` and using
/// `execute_batch_with_permissions`.
#[derive(Clone)]
pub struct ShamirDbHandler {
    pub(super) db: Arc<ShamirDb>,
    /// `None` means the handler was constructed without admin support;
    /// `CreateScramUser` requests will return `not_supported`.
    pub(super) admin: Option<AdminGlue>,
    /// Slow-query log threshold. Default disabled for unit tests; the
    /// boot path sets this from the operator's config.
    pub(super) slow_query: SlowQueryConfig,
    /// Server-side hard caps on per-batch resources.
    pub(super) query_limits: QueryLimitsCap,
    /// Server-side hard cap on per-interactive-tx staged bytes.
    pub(super) tx_limits: TxLimitsCap,
    /// Read/write mode of this node. `ReadOnly` rejects client writes
    /// (they must go to the leader). Default `ReadWrite`; until R1 this
    /// is always `ReadWrite` so behaviour is unchanged.
    pub(super) node_mode: NodeMode,
    /// Leader epoch for VR-style fencing (REPLICATION §5.2). Every
    /// `ReplResponse` carries this value so followers can detect a
    /// regression (stale leader). Default `1`; persistence + bump-on-
    /// promote land in R3.
    pub(super) leader_epoch: u64,
    /// Phase B — registry of open interactive (multi-call) transactions,
    /// shared across all clones of the handler (`Arc`), so a `TxExecute` on
    /// one dispatch finds the tx a prior `TxBegin` parked.
    pub(super) tx_registry: Arc<TxRegistry>,
    /// `SessionStore` access for ops that must kill OTHER live sessions of
    /// the calling user (currently only `ChangePasswordVerify`'s spec
    /// §12.5.3 session-kill step). `None` means the handler was built
    /// without it — the credential update in `ChangePasswordVerify` still
    /// lands, but the session-kill half is skipped (documented partial-fix
    /// fallback; see `ShamirDbHandler::with_session_store`).
    ///
    /// `SessionStore` itself lives at the connection/server layer
    /// (`server_launcher.rs` creates it once and passes it into
    /// `dispatch_request_view` as its own parameter for the §7.5 validity
    /// check) — this field is an ADDITIONAL reference threaded in at
    /// construction time so this handler can reach it too, without
    /// restructuring `dispatch_request_view`'s signature or the
    /// `RequestHandler` trait.
    pub(super) session_store: Option<Arc<SessionStore>>,
}

impl ShamirDbHandler {
    /// Construct a handler over a shared [`ShamirDb`] without admin support.
    /// Use [`Self::with_admin`] when SCRAM user creation should be possible.
    pub fn new(db: Arc<ShamirDb>) -> Self {
        Self {
            db,
            admin: None,
            slow_query: SlowQueryConfig::DISABLED,
            query_limits: QueryLimitsCap::UNLIMITED,
            tx_limits: TxLimitsCap::UNLIMITED,
            node_mode: NodeMode::default(),
            leader_epoch: 1,
            tx_registry: Arc::new(TxRegistry::new()),
            session_store: None,
        }
    }

    /// Construct a handler with admin (SCRAM user-creation) support.
    pub fn with_admin(db: Arc<ShamirDb>, admin: AdminGlue) -> Self {
        Self {
            db,
            admin: Some(admin),
            slow_query: SlowQueryConfig::DISABLED,
            query_limits: QueryLimitsCap::UNLIMITED,
            tx_limits: TxLimitsCap::UNLIMITED,
            node_mode: NodeMode::default(),
            leader_epoch: 1,
            tx_registry: Arc::new(TxRegistry::new()),
            session_store: None,
        }
    }

    /// Set the slow-query threshold (use [`SlowQueryConfig::from_ms`]).
    /// Returns `self` so the call can be chained after `with_admin`.
    pub fn with_slow_query(mut self, slow_query: SlowQueryConfig) -> Self {
        self.slow_query = slow_query;
        self
    }

    /// Set server-side hard caps on per-batch resources.
    pub fn with_query_limits(mut self, query_limits: QueryLimitsCap) -> Self {
        self.query_limits = query_limits;
        self
    }

    /// Set the per-interactive-tx staging byte cap.
    pub fn with_tx_limits(mut self, tx_limits: TxLimitsCap) -> Self {
        self.tx_limits = tx_limits;
        self
    }

    /// Set the node read/write mode. A `ReadOnly` node rejects client
    /// writes at the [`Self::execute`] gate (before they reach the engine).
    /// Default is `ReadWrite`.
    pub fn with_node_mode(mut self, mode: NodeMode) -> Self {
        self.node_mode = mode;
        self
    }

    /// Set the leader epoch for VR-style replication fencing (§5.2).
    /// Every `ReplResponse` carries this value; followers track the max
    /// epoch seen and reject a regression. Default `1`. Persistence and
    /// bump-on-promote are R3; for now this is a static field.
    pub fn with_leader_epoch(mut self, epoch: u64) -> Self {
        self.leader_epoch = epoch;
        self
    }

    /// Give this handler access to the server-wide [`SessionStore`] so
    /// `ChangePasswordVerify` can kill every OTHER live session of the
    /// calling user (spec §12.5.3) after a successful credential change.
    /// Without this, `SessionStore` — which is owned by the connection/
    /// server layer and passed to `dispatch_request_view` as its own
    /// parameter, not through the `RequestHandler` trait — is unreachable
    /// from inside `handle`. Default `None`: the credential update in
    /// `ChangePasswordVerify` still lands, but the session-kill step is
    /// skipped.
    pub fn with_session_store(mut self, session_store: Arc<SessionStore>) -> Self {
        self.session_store = Some(session_store);
        self
    }

    /// Reference to the underlying [`ShamirDb`] (for tests / admin glue).
    pub fn db(&self) -> &Arc<ShamirDb> {
        &self.db
    }

    /// Phase B Stage 6 — borrow the interactive-tx registry so the boot
    /// path can spawn the periodic reaper against it. The registry is
    /// already `Arc<TxRegistry>` internally; this returns a clone of that
    /// `Arc` so the reaper task and the handler share state.
    pub fn tx_registry(&self) -> Arc<TxRegistry> {
        Arc::clone(&self.tx_registry)
    }
}

impl RequestHandler for ShamirDbHandler {
    fn handle<'a>(
        &'a self,
        session: &'a Session,
        req: &'a [u8],
        conn: &'a ConnectionServices,
    ) -> shamir_connect::server::dispatch::HandlerFuture<'a> {
        Box::pin(async move {
            let request: DbRequest =
                rmp_serde::from_slice(req).map_err(|e| format!("invalid_request: {}", e))?;

            let response = match request {
                DbRequest::Ping => DbResponse::Pong,
                DbRequest::Execute {
                    query_version,
                    db,
                    batch,
                } => self.execute(session, query_version, &db, batch, conn).await,
                DbRequest::CreateScramUser {
                    name,
                    password,
                    roles,
                    hmac,
                } => {
                    create_scram_user(self.admin.as_ref(), session, name, password, roles, hmac)
                        .await
                }
                DbRequest::SetSuperuser { user, on, hmac } => {
                    set_superuser(self.admin.as_ref(), session, user, on, hmac).await
                }
                DbRequest::SetReplicator { user, on, hmac } => {
                    set_replicator(self.admin.as_ref(), session, user, on, hmac).await
                }

                // --- Phase B: interactive (multi-call) transactions ---
                DbRequest::TxBegin {
                    query_version,
                    db,
                    repo,
                    isolation,
                } => {
                    self.tx_begin(session, query_version, &db, &repo, isolation)
                        .await
                }
                DbRequest::TxExecute {
                    query_version,
                    db,
                    tx_handle,
                    batch,
                } => {
                    self.tx_execute(session, query_version, &db, tx_handle, batch)
                        .await
                }
                DbRequest::TxCommit { db, tx_handle } => {
                    self.tx_commit(session, &db, tx_handle).await
                }
                DbRequest::TxRollback { db, tx_handle } => {
                    self.tx_rollback(session, &db, tx_handle).await
                }
                // REPLICATION §5 — privileged pull-API (R0-b).
                // Role + per-repo authorisation enforced inside `handle_repl`.
                DbRequest::Repl(repl_req) => {
                    DbResponse::Repl(self.handle_repl(session, repl_req).await)
                }

                // changePassword (spec §12.5) — see `db_handler::admin`.
                DbRequest::ChangePasswordChallenge { client_nonce_cp } => {
                    change_password_challenge(self.admin.as_ref(), session, client_nonce_cp).await
                }
                DbRequest::ChangePasswordVerify {
                    client_proof_old,
                    new_salt,
                    new_stored_key,
                    new_server_key,
                } => {
                    change_password_verify(
                        self.admin.as_ref(),
                        self.session_store.as_ref(),
                        session,
                        client_proof_old,
                        new_salt,
                        new_stored_key,
                        new_server_key,
                    )
                    .await
                }
            };

            rmp_serde::to_vec_named(&response).map_err(|e| format!("encode_error: {}", e))
        })
    }
}

impl ShamirDbHandler {
    /// Run the version check + admin gate, then forward to
    /// [`ShamirDb::execute`] on the current Tokio worker.
    pub(super) async fn execute(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        mut batch: BatchRequest,
        conn: &ConnectionServices,
    ) -> DbResponse {
        // Query-language version dispatch — fast reject before any DB work.
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }

        // Server-side cap on `BatchRequest.limits`. Client may shrink any
        // field, but cannot exceed the operator-configured cap. Applied
        // BEFORE the planner sees the batch so over-cap requests fail
        // through `BatchError::TooManyQueries` etc. with the cap as the
        // reported max — not the client-supplied value.
        batch.limits.max_result_size = batch
            .limits
            .max_result_size
            .min(self.query_limits.max_result_size_bytes);
        batch.limits.max_execution_time_secs = batch
            .limits
            .max_execution_time_secs
            .min(self.query_limits.max_execution_time_secs);
        batch.limits.max_queries = batch
            .limits
            .max_queries
            .min(self.query_limits.max_queries_per_batch);

        // Admin / auth gate. An explicit 4-op allowlist (`List`,
        // `AccessTree`, `DescribeTable`, `GetTableSchema` — task #553, see
        // `is_coarse_admin_gate_exempt`) is exempted from this coarse
        // block; each still runs its own real per-table/per-path
        // authorization further down the stack.
        if !session.permissions.is_superuser {
            for (alias, entry) in &batch.queries {
                if entry.op.is_admin() && !is_coarse_admin_gate_exempt(&entry.op) {
                    return DbResponse::Error {
                        code: "permission_denied".into(),
                        message: format!("query '{}' requires superuser (admin/auth op)", alias),
                    };
                }
            }
        }

        // Read-only replica gate. A `ReadOnly` node rejects any client write
        // — client writes must go to the leader. Read/introspection ops pass
        // through. `is_write()` is exhaustive, so any new `BatchOp` variant
        // is automatically classified and the author must decide read vs
        // write. This is a no-op when `node_mode == ReadWrite` (the default).
        // R1: include leader_addr for client redirect.
        if self.node_mode == NodeMode::ReadOnly {
            for (alias, entry) in &batch.queries {
                if entry.op.is_write() {
                    return DbResponse::Error {
                        code: "read_only_replica".into(),
                        message: format!(
                            "query '{}' is a write; this node is a read-only replica",
                            alias
                        ),
                    };
                }
            }
        }

        // Destructive-op HMAC gate. Every drop_* op must carry an
        // `hmac` field whose tag covers the canonical bytes for
        // that op, keyed by the per-session derived key. This is
        // not an authentication gate — TLS+SCRAM already proves
        // the caller — it's a "did you mean it" guard: the client
        // could not produce the tag by accident.
        if let Err((alias, code, message)) = check_destructive_hmacs(session, db_name, &batch) {
            return DbResponse::Error {
                code: code.into(),
                message: format!("query '{}': {}", alias, message),
            };
        }

        let actor = session_actor(session);
        let exec_result = self.db.execute_as(actor.clone(), db_name, &batch).await;
        match exec_result {
            Ok(mut response) => {
                // Slow-query logging: WARN line for batches whose total
                // execution time exceeds the configured threshold. Useful
                // for spotting unindexed queries in production. Threshold
                // = 0 disables (e.g. in unit tests).
                if self.slow_query.threshold_us > 0
                    && response.execution_time_us > self.slow_query.threshold_us
                {
                    tracing::warn!(
                        elapsed_us = response.execution_time_us,
                        threshold_us = self.slow_query.threshold_us,
                        db = db_name,
                        queries = batch.queries.len(),
                        request_id = ?response.id,
                        "slow query",
                    );
                }
                self.persist_table_lifecycle(db_name, &batch);
                subscribe_handler::activate_subscriptions(
                    conn,
                    &self.db,
                    db_name,
                    &batch,
                    &mut response,
                    actor.clone(),
                );
                DbResponse::Batch { response }
            }
            Err(e) => DbResponse::Error {
                code: error_code(&e).to_string(),
                message: e.to_string(),
            },
        }
    }

    /// Walk through `batch.queries` and record CreateTable/DropTable ops in
    /// the persistent registry. No-op when `AdminGlue::tables_registry` is
    /// `None`. Failures here are logged but never break the request — the
    /// in-memory state already reflects the change; the registry is just a
    /// boot-replay aid.
    pub(super) fn persist_table_lifecycle(&self, db_name: &str, batch: &BatchRequest) {
        let Some(admin) = &self.admin else { return };
        let Some(reg) = &admin.tables_registry else {
            return;
        };
        for entry in batch.queries.values() {
            match &entry.op {
                BatchOp::CreateTable(op) => {
                    if let Err(e) = reg.add(db_name, &op.repo, &op.create_table) {
                        tracing::warn!(?e, "tables_registry add failed");
                    }
                }
                BatchOp::DropTable(op) => {
                    if let Err(e) = reg.remove(db_name, &op.repo, &op.drop_table) {
                        tracing::warn!(?e, "tables_registry remove failed");
                    }
                }
                _ => {}
            }
        }
    }
}

/// Classification of a [`BatchError`] for the wire `code` tag.
///
/// When the error carries a structured `code` (DDL / admin errors set one
/// since §5 error-codes), that code is returned verbatim. Unclassified
/// errors fall back to heuristic string-matching for backward compat.
pub(super) fn error_code(e: &BatchError) -> &str {
    match e {
        BatchError::TooManyQueries { .. }
        | BatchError::TooDeep { .. }
        | BatchError::TooManyIterations { .. } => "limits",
        BatchError::CircularDependency { .. }
        | BatchError::UnknownAlias { .. }
        | BatchError::AfterPathIgnored { .. } => "validation",
        BatchError::Timeout { .. } => "timeout",
        BatchError::LockTimeout { .. } => "lock_timeout",
        BatchError::QueryError {
            alias,
            message,
            code,
        } => {
            // Prefer structured code when present.
            if let Some(c) = code {
                return c.as_str();
            }
            // Legacy heuristic for untagged errors.
            if alias.is_empty() && message.contains("not found") {
                "unknown_db"
            } else if message.starts_with("access denied:") {
                "access_denied"
            } else {
                "query"
            }
        }
        BatchError::CrossRepoNotSupported { .. } => "tx_cross_repo_not_supported",
        BatchError::NestingTooDeep { .. } => "nesting_too_deep",
    }
}
