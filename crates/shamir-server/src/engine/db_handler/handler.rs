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

use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionStore};

use shamir_db::access::Actor;
use shamir_db::query::batch::{BatchError, BatchOp, BatchRequest};
use shamir_db::ShamirDb;
use shamir_query_types::batch::BatchTreeVisitor;
use shamir_query_types::hmac as canon;

use crate::byte_budget::{stash_guard, stash_serialized_response, ByteBudget};

// Wire DTOs are defined once in `shamir-query-types::wire` so that the
// SDK (`shamir-client`) and the server share the same definition. Re-
// exported here so existing `shamir_server::db_handler::DbRequest` import
// paths keep resolving.
pub use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::cursor_registry::CursorRegistry;
use crate::tx_registry::TxRegistry;

use super::admin::{
    change_password_challenge, change_password_verify, check_destructive_hmacs, create_scram_user,
    is_coarse_admin_gate_exempt, set_replicator, set_superuser, AdminGlue,
};
use super::config::{CursorLimitsCap, NodeMode, QueryLimitsCap, SlowQueryConfig, TxLimitsCap};
use super::cursor_handlers::error_response;
use super::cursor_support::authorize_cursor_read;
use super::repo_audit;
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
    /// Per-connection audit writer, attached by the request boundary.
    pub(super) audit: Option<Arc<AuditChainWriter>>,
    pub(super) transactions: crate::config::TransactionsConfig,
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
    /// RI-15 — global cap on the SUM of in-flight response bytes across
    /// every concurrently-executing batch/connection. Default
    /// [`ByteBudget::unbounded`] (matches pre-RI-15 behavior: only the
    /// per-batch `query_limits.max_result_size_bytes` cap applies).
    pub(super) byte_budget: ByteBudget,
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
    /// FG-5b — registry of open server-side result cursors, shared across
    /// all clones of the handler (`Arc`), so a `FetchNext`/`CancelCursor`
    /// on one dispatch finds the cursor a prior `CreateCursor` parked.
    pub(super) cursor_registry: Arc<CursorRegistry>,
    /// FG-5b — server-side hard caps on result cursors (per-session count,
    /// idle-timeout eviction).
    pub(super) cursor_limits: CursorLimitsCap,
    /// FG-5b — monotonic source of fresh `CursorId` values. A plain
    /// `AtomicU64` (not the tx registry's `RepoTxGate::fresh_tx_id`,
    /// which is per-repo and async) since cursor ids are a
    /// server-handler-scoped concept, not an engine one.
    pub(super) next_cursor_id: Arc<std::sync::atomic::AtomicU64>,
    /// `SessionStore` access for ops that must kill OTHER live sessions of
    /// the calling user (currently only `ChangePasswordVerify`'s spec
    /// §12.5.3 session-kill step). `None` means the handler was built
    /// without it — the credential update in `ChangePasswordVerify` still
    /// lands, but the session-kill half is skipped (documented partial-fix
    /// fallback; see `ShamirDbHandler::with_session_store`).
    ///
    /// `SessionStore` itself lives at the connection/server layer
    /// (`server_launcher/launch.rs` creates it once and passes it into
    /// `dispatch_request_view` as its own parameter for the §7.5 validity
    /// check) — this field is an ADDITIONAL reference threaded in at
    /// construction time so this handler can reach it too, without
    /// restructuring `dispatch_request_view`'s signature or the
    /// `RequestHandler` trait.
    pub(super) session_store: Option<Arc<SessionStore>>,
}

impl ShamirDbHandler {
    pub fn with_audit_writer(mut self, audit: Arc<AuditChainWriter>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Construct a handler over a shared [`ShamirDb`] without admin support.
    /// Use [`Self::with_admin`] when SCRAM user creation should be possible.
    pub fn new(db: Arc<ShamirDb>) -> Self {
        Self {
            db,
            audit: None,
            transactions: crate::config::TransactionsConfig::default(),
            admin: None,
            slow_query: SlowQueryConfig::DISABLED,
            query_limits: QueryLimitsCap::UNLIMITED,
            tx_limits: TxLimitsCap::UNLIMITED,
            byte_budget: ByteBudget::unbounded(),
            node_mode: NodeMode::default(),
            leader_epoch: 1,
            tx_registry: Arc::new(TxRegistry::new()),
            cursor_registry: Arc::new(CursorRegistry::new()),
            cursor_limits: CursorLimitsCap::DEFAULT,
            next_cursor_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            session_store: None,
        }
    }

    /// Construct a handler with admin (SCRAM user-creation) support.
    pub fn with_admin(db: Arc<ShamirDb>, admin: AdminGlue) -> Self {
        Self {
            db,
            audit: None,
            transactions: crate::config::TransactionsConfig::default(),
            admin: Some(admin),
            slow_query: SlowQueryConfig::DISABLED,
            query_limits: QueryLimitsCap::UNLIMITED,
            tx_limits: TxLimitsCap::UNLIMITED,
            byte_budget: ByteBudget::unbounded(),
            node_mode: NodeMode::default(),
            leader_epoch: 1,
            tx_registry: Arc::new(TxRegistry::new()),
            cursor_registry: Arc::new(CursorRegistry::new()),
            cursor_limits: CursorLimitsCap::DEFAULT,
            next_cursor_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
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

    /// Set the server-side result-cursor caps (per-session count,
    /// idle-timeout). Default [`CursorLimitsCap::DEFAULT`].
    pub fn with_cursor_limits(mut self, cursor_limits: CursorLimitsCap) -> Self {
        self.cursor_limits = cursor_limits;
        self
    }

    /// Set the server-wide in-flight response-byte budget (RI-15).
    pub fn with_byte_budget(mut self, byte_budget: ByteBudget) -> Self {
        self.byte_budget = byte_budget;
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

    /// FG-5b — borrow the cursor registry so the boot path can spawn the
    /// periodic reaper against it. Mirrors [`Self::tx_registry`].
    pub fn cursor_registry(&self) -> Arc<CursorRegistry> {
        Arc::clone(&self.cursor_registry)
    }

    /// FG-5b — mint a fresh, process-unique cursor id.
    pub(super) fn next_cursor_id(&self) -> u64 {
        self.next_cursor_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
            let request: DbRequest = match shamir_query_types::validate_request_alias_caps(req)
                .and_then(|()| rmp_serde::from_slice(req))
            {
                Ok(request) => request,
                Err(e) => {
                    let response = DbResponse::Error {
                        code: "invalid_request".into(),
                        message: e.to_string(),
                    };
                    return rmp_serde::to_vec_named(&response)
                        .map_err(|encode_err| format!("response encode: {}", encode_err));
                }
            };

            let top_level_canonical = match canon::canonical_db_request(&request) {
                Ok(bytes) => Some(bytes),
                Err(canon::CanonicalOpError::UnsupportedOperation) => None,
                Err(canon::CanonicalOpError::Encoding(_)) => {
                    let response = DbResponse::Error {
                        code: "hmac_required".into(),
                        message: "request canonical encoding failed".into(),
                    };
                    return rmp_serde::to_vec_named(&response)
                        .map_err(|encode_err| format!("response encode: {}", encode_err));
                }
            };
            let response = match request {
                DbRequest::Ping => DbResponse::Pong,
                DbRequest::WaitForVisibility {
                    query_version,
                    db,
                    repo,
                    version,
                    visibility_wait,
                } => {
                    self.wait_for_visibility(
                        session,
                        query_version,
                        &db,
                        &repo,
                        version,
                        visibility_wait,
                    )
                    .await
                }
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
                    create_scram_user(
                        self.admin.as_ref(),
                        session,
                        name,
                        password,
                        roles,
                        hmac,
                        top_level_canonical,
                    )
                    .await
                }
                DbRequest::SetSuperuser { user, on, hmac } => {
                    set_superuser(
                        self.admin.as_ref(),
                        session,
                        user,
                        on,
                        hmac,
                        top_level_canonical,
                    )
                    .await
                }
                DbRequest::SetReplicator { user, on, hmac } => {
                    set_replicator(
                        self.admin.as_ref(),
                        session,
                        user,
                        on,
                        hmac,
                        top_level_canonical,
                    )
                    .await
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
                DbRequest::TxCommit {
                    query_version,
                    visibility_wait,
                    db,
                    tx_handle,
                } => {
                    if let Err(e) = crate::version::check_query_lang(query_version) {
                        DbResponse::Error {
                            code: "unsupported_query_version".into(),
                            message: e.to_string(),
                        }
                    } else {
                        match visibility_wait {
                            None => self.tx_commit(session, &db, tx_handle).await,
                            Some(wait) => {
                                self.tx_commit_with_options(session, &db, tx_handle, Some(wait))
                                    .await
                            }
                        }
                    }
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
                DbRequest::ChangePasswordVerify(secrets) => {
                    change_password_verify(
                        self.admin.as_ref(),
                        self.session_store.as_ref(),
                        session,
                        secrets.client_proof_old,
                        secrets.new_salt,
                        secrets.new_stored_key,
                        secrets.new_server_key,
                    )
                    .await
                }

                // --- FG-5b: server-side result cursors ---
                // See `crate::cursor_registry` and `db_handler::cursor_handlers`
                // for the registry + snapshot-pinning + keyset-pagination
                // implementation this dispatches into.
                DbRequest::CreateCursor {
                    query_version,
                    db,
                    query,
                    page_size,
                } => {
                    self.create_cursor(session, query_version, &db, query, page_size)
                        .await
                }
                DbRequest::FetchNext {
                    cursor_id,
                    page_size,
                } => self.fetch_next(session, cursor_id, page_size).await,
                DbRequest::CancelCursor { cursor_id } => {
                    self.cancel_cursor(session, cursor_id).await
                }
                DbRequest::GetDdlOpStatus {
                    db,
                    repo,
                    table,
                    op_id,
                } => {
                    self.get_ddl_op_status(session, &db, &repo, &table, &op_id)
                        .await
                }
            };

            // CR-B2: `ShamirDbHandler::execute` already serialized this
            // exact `response` value once (to shrink its upfront RI-15
            // reservation down to the actual size) and stashed those bytes
            // via `stash_serialized_response`. Reuse them here instead of
            // encoding a second time — every OTHER dispatch arm above
            // (`Ping`, `ChangePassword*`, cursor ops, etc.) never stashes
            // anything, so this naturally falls through to the fresh
            // serialize for them.
            if let Some(bytes) = crate::byte_budget::take_stashed_serialized_response() {
                return Ok(bytes);
            }

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

        // Resolve all explicit policies before admission or any writes, including
        // nested bodies whose outcomes cannot carry a bounded commit receipt.
        let policy = (|| {
            let wait = self
                .transactions
                .resolve_commit_wait(batch.visibility_wait)?;
            for visit in BatchTreeVisitor::new(&batch.queries) {
                let nested = match &visit.entry.op {
                    shamir_query_types::batch::BatchOp::Batch(sub) => sub.batch.visibility_wait,
                    shamir_query_types::batch::BatchOp::ForEach(fe) => fe.batch.visibility_wait,
                    _ => None,
                };
                if let Some(nested) = nested {
                    self.transactions.resolve_wait(nested)?;
                    if batch.transactional
                        && matches!(
                            nested,
                            shamir_query_types::batch::VisibilityWait::Bounded { .. }
                        )
                    {
                        return Err(shamir_db::DbError::VisibilityWaitInvalid {
                            reason:
                                "bounded visibility is unsupported by nested Batch/ForEach bodies"
                                    .into(),
                        });
                    }
                }
            }
            Ok(wait)
        })();
        match policy {
            Ok(wait) => batch.visibility_wait = Some(wait),
            Err(e) => {
                return DbResponse::Error {
                    code: "visibility_wait_invalid".into(),
                    message: e.to_string(),
                };
            }
        }

        // Admin / auth gate. An explicit 4-op allowlist (`List`,
        // `AccessTree`, `DescribeTable`, `GetTableSchema` — task #553, see
        // `is_coarse_admin_gate_exempt`) is exempted from this coarse
        // block; each still runs its own real per-table/per-path
        // authorization further down the stack.
        if !session.permissions.is_superuser {
            for visit in
                BatchTreeVisitor::new(&batch.queries).filter(|visit| visit.body_depth().is_none())
            {
                let (alias, entry) = (visit.alias, visit.entry);
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
            for visit in
                BatchTreeVisitor::new(&batch.queries).filter(|visit| visit.body_depth().is_none())
            {
                let (alias, entry) = (visit.alias, visit.entry);
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

        // HMAC policy gate. Every operation classified as gated by the
        // canonical HMAC policy must carry a tag covering its canonical
        // bytes, keyed by the per-session derived key. This is not an
        // authentication gate — TLS+SCRAM already proves the caller — it's
        // a "did you mean it" guard: the client could not produce the tag
        // by accident. BatchTreeVisitor checks static ForEach template bodies
        // before execution; signatures cover unresolved template operations,
        // not per-iteration values injected through bind_row.
        if let Err((alias, code, message)) = check_destructive_hmacs(session, db_name, &batch) {
            return DbResponse::Error {
                code: code.into(),
                message: format!("query '{}': {}", alias, message),
            };
        }

        // RI-15 (CR-B2) — reserve an upfront, pessimistic estimate from the
        // global in-flight response-byte budget BEFORE execution begins,
        // using min(request result size, server result cap) as the estimate,
        // without modifying the request. This is what
        // actually bounds EXECUTION-time memory (not just write-path
        // residency): N concurrent requests can no longer all run to
        // completion and materialize their full result before any of them
        // is throttled — the (N+1)-th request now blocks HERE, before
        // `execute_with_policy` ever runs, once the budget is saturated.
        //
        // Deliberately placed AFTER every cheap, budget-free fast-reject
        // above (version check, admin gate, read-only gate, HMAC gate) —
        // those should stay free of any budget interaction. The guard is
        // held locally for the rest of this function and shrunk to the
        // ACTUAL serialized size once the final response is known (see the
        // single tail below), rather than stashed immediately: stashing
        // happens exactly once, at the end, same as before this change.
        let reserve_bytes = batch
            .limits
            .max_result_size
            .min(self.query_limits.max_result_size_bytes);
        let mut budget_guard = if self.byte_budget.cap().is_some() {
            Some(self.byte_budget.acquire(reserve_bytes).await)
        } else {
            None
        };

        let actor = session_actor(session);
        let repo_observer = self
            .audit
            .as_ref()
            .map(|audit| repo_audit::repo_audit_observer(audit.clone(), session));
        let exec_result = self
            .db
            .execute_with_policy_and_repo_observer(
                actor.clone(),
                db_name,
                &batch,
                self.execution_policy(),
                repo_observer,
            )
            .await;
        let final_response = match exec_result {
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
            Err(e) => error_response(&e),
        };

        // RI-15 (CR-B2) — single convergence point for BOTH the success and
        // error paths: serialize the final `DbResponse` envelope exactly
        // ONCE, shrink the upfront reservation down to that actual size
        // (an error response's tiny actual size still needs its pessimistic
        // over-reservation narrowed back down), then stash both the guard
        // and the already-serialized bytes so `RequestHandler::handle`
        // reuses them instead of encoding the SAME response a second time.
        //
        // The guard is stashed in a task-local (`byte_budget::stash_guard`)
        // rather than returned from this function: `execute`'s return type
        // is `DbResponse` (asserted on directly by tests) and the caller,
        // `RequestHandler::handle`, has a fixed `Result<Vec<u8>, String>`
        // signature owned by `shamir-connect`. `connection::request_loop`
        // retrieves the guard with `byte_budget::take_stashed_guard` right
        // after the dispatch future resolves and rides it inside
        // `WriterMsg::{Reply,ReplyAndClose}` so the release happens in the
        // writer task, after the socket write completes — never in this
        // dispatch task.
        if let Ok(bytes) = rmp_serde::to_vec_named(&final_response) {
            if bytes.len() > reserve_bytes {
                // Includes arbitrary error strings, not just successful payloads.
                // Release before returning; never stash the rejected payload.
                drop(budget_guard);
                return DbResponse::Error {
                    code: "result_too_large".to_string(),
                    message: format!(
                        "encoded response is {} bytes; maximum is {reserve_bytes}",
                        bytes.len(),
                    ),
                };
            }
            if let Some(guard) = budget_guard.as_mut() {
                guard.shrink_to(bytes.len());
            }
            if let Some(guard) = budget_guard {
                stash_guard(guard);
            }
            stash_serialized_response(bytes);
        }

        final_response
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
/// since §5 error-codes; the engine's implicit-commit receipt mapping sets
/// the commit-admission codes `wal_requires_reopen` /
/// `wal_retention_exceeded`), that code is returned verbatim. Unclassified
/// errors fall back to heuristic string-matching for backward compat.
pub(super) fn error_code(e: &BatchError) -> &str {
    match e {
        BatchError::LimitExceeded { kind, .. } => match kind {
            shamir_query_types::batch::LimitKind::PageSize
            | shamir_query_types::batch::LimitKind::Offset => "pagination_too_large",
            shamir_query_types::batch::LimitKind::Work
            | shamir_query_types::batch::LimitKind::TotalQueries => "work_budget_exceeded",
            shamir_query_types::batch::LimitKind::ResultBytes => "result_too_large",
            shamir_query_types::batch::LimitKind::Depth => "nesting_too_deep",
            shamir_query_types::batch::LimitKind::Iterations => "too_many_iterations",
        },
        BatchError::TooManyIterations { .. } => "too_many_iterations",
        BatchError::TooManyQueries { .. }
        | BatchError::TooDeep { .. }
        | BatchError::ExecutionTimedOut { .. } => "limits",
        BatchError::CircularDependency { .. }
        | BatchError::UnknownAlias { .. }
        | BatchError::AfterPathIgnored { .. }
        | BatchError::InvalidWhenFilter { .. }
        | BatchError::InvalidCondCondition { .. } => "validation",
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
        // FG-5b: wire error codes for the cursor protocol.
        BatchError::CursorNotFound { .. } => "cursor_not_found",
        BatchError::CursorExpired { .. } => "cursor_expired",
        BatchError::CursorLimitExceeded { .. } => "cursor_limit_exceeded",
        // FG-5b: `CreateCursor` rejects AsOf/History queries outright (see
        // the variant's doc comment for the scope-cut rationale).
        BatchError::CursorTemporalNotSupported => "cursor_temporal_not_supported",
        // CR-B5: `CreateCursor` rejects `with_version = true` outright (see
        // the variant's doc comment — a cursor's AsOf-pinned reads hard-code
        // `versions: None`, so honoring the flag would silently drop it).
        BatchError::CursorWithVersionNotSupported => "cursor_with_version_not_supported",
        // CR-A3: `page_size == 0` (infinite-loop hazard) or `page_size` above
        // `max_cursor_page_size` (unbounded materialization hazard).
        BatchError::InvalidPageSize { .. } => "invalid_page_size",
        // CR-A5: a cursor page's serialized size exceeded
        // `max_result_size_bytes`; rejected outright, never truncated.
        BatchError::CursorPageTooLarge { .. } => "cursor_page_too_large",
        BatchError::AsOfVisibilityTimeout { .. } => "as_of_visibility_timeout",
        // RV-WAL-3B29232E: implicit single-op commit whose WAL outcome was
        // unknown after queueing — stable code; the attempted commit_version
        // rides in the typed error payload, no success claim.
        BatchError::WalOutcomeUnknown { .. } => "wal_requires_reopen",
        // S5c-X PR8b: read refused — replication destination in
        // `resync_required` quarantine; non-retryable until reseed.
        BatchError::ResyncRequired { .. } => "resync_required",
    }
}

impl ShamirDbHandler {
    /// Handle `DbRequest::GetDdlOpStatus` — poll a DDL operation's status by ID.
    ///
    /// #1064 (P0 SECURITY): this poll previously ran with NO authorization at
    /// all — any authenticated client that knew (or guessed, or found in a
    /// log/metric) an `op_id` could read another actor's DDL status,
    /// including `Failed.detail` internal error text, and use the
    /// found/not-found distinction as a db/repo/table existence oracle.
    /// Gated the same way `create_cursor` gates a cursor read
    /// (`authorize_cursor_read`: `Action::Read` on the `Database`, then on
    /// the `Table`) — reused directly rather than duplicated, since a DDL
    /// status poll is semantically "read this table's operational state."
    /// Authorizing BEFORE any table/repo resolution (using the raw name
    /// strings, exactly like `create_cursor` does) is what keeps an
    /// unauthorized caller's response indistinguishable from a genuine
    /// not-found — `self.db.get_ddl_op_status` (which does the actual
    /// resolution and can reveal unknown-db/unknown-repo distinctly) is
    /// never even called unless authorization already succeeded.
    async fn get_ddl_op_status(
        &self,
        session: &Session,
        db: &str,
        repo: &str,
        table: &str,
        op_id_str: &str,
    ) -> DbResponse {
        let actor = session_actor(session);
        if let Err(e) = authorize_cursor_read(&self.db, &actor, db, repo, table).await {
            return error_response(&e);
        }

        match self.db.get_ddl_op_status(db, repo, table, op_id_str).await {
            Ok(status) => DbResponse::DdlOpStatus { status },
            Err(e) => DbResponse::Error {
                code: match e {
                    shamir_db::DbError::Validation(_) => "validation",
                    shamir_db::DbError::NotFound(_) => "not_found",
                    _ => "query",
                }
                .to_string(),
                message: format!("Failed to read DDL op status: {}", e),
            },
        }
    }
}
