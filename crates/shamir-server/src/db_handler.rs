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
//!     - auth ops (CreateUser / DropUser / CreateRole / DropRole / GrantRole /
//!       RevokeRole),
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
//! # Async bridge — HIGH-7
//!
//! `RequestHandler::handle` is sync; [`ShamirDb::execute`] is async.
//! `crate::connection::request_loop` wraps the entire synchronous handler
//! chain in [`tokio::task::spawn_blocking`] so the call runs on the
//! blocking-task pool (default 512 threads) rather than parking a runtime
//! worker for the full batch duration — without this an authenticated
//! peer issuing slow batches from a handful of connections could starve
//! every other connection (cap on concurrent in-flight batches was
//! previously `#worker_threads`). Inside the blocking thread,
//! [`run_blocking`] spawns the async DB future back onto the runtime and
//! waits on a `std::sync::mpsc::Receiver`; the future therefore makes
//! progress concurrently with other connections' I/O on the worker pool.

use std::sync::Arc;

use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::Session;

use shamir_db::query::batch::{BatchError, BatchOp, BatchRequest};
use shamir_db::ShamirDb;

// Wire DTOs are defined once in `shamir-query-types::wire` so that the
// SDK (`shamir-client`) and the server share the same definition. Re-
// exported here so existing `shamir_server::db_handler::DbRequest` import
// paths keep resolving.
pub use shamir_query_types::wire::{DbRequest, DbResponse};

use shamir_connect::common::crypto::random_array;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

use crate::tables_registry::TablesRegistry;
use crate::user_directory::RedbUserDirectory;
use crate::version::check_query_lang;

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

/// Optional admin glue — supplied by the boot path so admin ops that
/// require server-side state (the SCRAM user directory + KDF cost
/// parameters + the wire-tables persistence registry) can run. Tests
/// that don't need any of these omit it via `ShamirDbHandler::new`.
#[derive(Clone)]
pub struct AdminGlue {
    /// Directory that stores SCRAM-authenticatable users.
    pub user_dir: Arc<RedbUserDirectory>,
    /// KDF defaults applied to newly created users so they can log in
    /// against the same listener policy.
    pub kdf: KdfParams,
    /// Tracks tables created/dropped over the wire so the boot path can
    /// re-register them on restart. `None` means "don't persist table
    /// changes" — fine for in-memory test setups, wrong for production.
    pub tables_registry: Option<Arc<TablesRegistry>>,
}

/// Per-batch slow-query threshold (in microseconds, matching
/// `BatchResponse::execution_time_us`). `0` disables the warning.
/// Set on the handler at boot from `[logging] slow_query_threshold_ms`.
#[derive(Debug, Clone, Copy)]
pub struct SlowQueryConfig {
    pub threshold_us: u64,
}

impl SlowQueryConfig {
    pub const DISABLED: Self = Self { threshold_us: 0 };
    pub fn from_ms(ms: u64) -> Self {
        Self {
            threshold_us: ms.saturating_mul(1_000),
        }
    }
}

/// Server-side hard caps on `BatchRequest.limits`. Applied as a max:
/// the client's payload values are clamped DOWN to these caps before
/// the batch is dispatched into `ShamirDb::execute`.
///
/// Set on the handler at boot from `[security.query_limits]`. Tests that
/// don't care about resource limits use [`Self::UNLIMITED`].
#[derive(Debug, Clone, Copy)]
pub struct QueryLimitsCap {
    pub max_result_size_bytes: usize,
    pub max_execution_time_secs: u64,
    pub max_queries_per_batch: usize,
}

impl QueryLimitsCap {
    /// Effectively-no-cap defaults — for unit tests. Matches `BatchLimits::default()`.
    pub const UNLIMITED: Self = Self {
        max_result_size_bytes: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_queries_per_batch: usize::MAX,
    };
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
    db: Arc<ShamirDb>,
    /// `None` means the handler was constructed without admin support;
    /// `CreateScramUser` requests will return `not_supported`.
    admin: Option<AdminGlue>,
    /// Slow-query log threshold. Default disabled for unit tests; the
    /// boot path sets this from the operator's config.
    slow_query: SlowQueryConfig,
    /// Server-side hard caps on per-batch resources.
    query_limits: QueryLimitsCap,
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
        }
    }

    /// Construct a handler with admin (SCRAM user-creation) support.
    pub fn with_admin(db: Arc<ShamirDb>, admin: AdminGlue) -> Self {
        Self {
            db,
            admin: Some(admin),
            slow_query: SlowQueryConfig::DISABLED,
            query_limits: QueryLimitsCap::UNLIMITED,
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

    /// Reference to the underlying [`ShamirDb`] (for tests / admin glue).
    pub fn db(&self) -> &Arc<ShamirDb> {
        &self.db
    }
}

impl RequestHandler for ShamirDbHandler {
    fn handle(&self, session: &Session, req: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let request: DbRequest =
            rmp_serde::from_slice(req).map_err(|e| format!("invalid_request: {}", e))?;

        let response = match request {
            DbRequest::Ping => DbResponse::Pong,
            DbRequest::Execute {
                query_version,
                db,
                batch,
            } => self.execute(session, query_version, &db, batch),
            DbRequest::CreateScramUser {
                name,
                password,
                roles,
            } => self.create_scram_user(session, name, password, roles),
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
        mut batch: BatchRequest,
    ) -> DbResponse {
        // Query-language version dispatch — fast reject before any DB work.
        if let Err(e) = check_query_lang(query_version) {
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

        // Admin / auth gate.
        if !session.permissions.is_superuser {
            for (alias, entry) in &batch.queries {
                if entry.op.is_admin() {
                    return DbResponse::Error {
                        code: "permission_denied".into(),
                        message: format!("query '{}' requires superuser (admin/auth op)", alias),
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

        // `run_blocking` now requires a `'static` future (it spawns the
        // future onto the runtime instead of driving it on the current
        // thread). The batch is already owned (passed by value); we
        // clone the shared `ShamirDb` Arc and own the db name string,
        // then move them into an `async move` block that hands the batch
        // back to us so the post-exec slow-query log and persistence
        // registry still see the request.
        let db = self.db.clone();
        let db_name_owned = db_name.to_string();
        let (batch, exec_result) = run_blocking(async move {
            let result = db.execute(&db_name_owned, &batch).await;
            (batch, result)
        });
        match exec_result {
            Ok(response) => {
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
                        request_id = %response.id,
                        "slow query",
                    );
                }
                self.persist_table_lifecycle(db_name, &batch);
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
    fn persist_table_lifecycle(&self, db_name: &str, batch: &BatchRequest) {
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

    /// Create a SCRAM-authenticatable user. Server-side Argon2id is run
    /// inside `block_in_place` to keep the Tokio worker responsive.
    fn create_scram_user(
        &self,
        session: &Session,
        name: String,
        password: String,
        roles: Vec<String>,
    ) -> DbResponse {
        if !session.permissions.is_superuser {
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
        let derived =
            match tokio::task::block_in_place(|| DerivedKeys::derive(&pw_buf, &salt, &admin.kdf)) {
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
                let code = if msg.contains("exists") {
                    "user_exists"
                } else {
                    "query"
                };
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

/// Walk the batch and verify the `hmac` tag on every destructive op.
///
/// Returns `Err((alias, code, message))` on the first failure
/// where `code` is one of:
///   * `"hmac_required"` — the field is missing on a destructive op,
///   * `"hmac_mismatch"` — the field is present but the tag doesn't
///     match the recomputed value for this op + this session.
///
/// Non-destructive ops pass through untouched. Auth check has
/// already happened above; this gate runs strictly after that.
fn check_destructive_hmacs(
    session: &Session,
    db_name: &str,
    batch: &BatchRequest,
) -> Result<(), (String, &'static str, String)> {
    use shamir_query_types::hmac as canon;

    // Lazy derive only when there's at least one destructive op.
    let mut key_opt: Option<[u8; 32]> = None;
    let key = |k: &mut Option<[u8; 32]>| -> [u8; 32] {
        if let Some(v) = *k {
            return v;
        }
        let derived = session.hmac_key();
        *k = Some(derived);
        derived
    };

    for (alias, entry) in &batch.queries {
        let (canonical, supplied): (Vec<u8>, Option<&String>) = match &entry.op {
            BatchOp::DropDb(op) => (canon::canonical_drop_db(&op.drop_db), op.hmac.as_ref()),
            BatchOp::DropRepo(op) => (
                canon::canonical_drop_repo(db_name, &op.drop_repo),
                op.hmac.as_ref(),
            ),
            BatchOp::DropTable(op) => (
                canon::canonical_drop_table(db_name, &op.repo, &op.drop_table),
                op.hmac.as_ref(),
            ),
            BatchOp::DropIndex(op) => (
                canon::canonical_drop_index(
                    db_name,
                    &op.repo,
                    &op.table,
                    &op.drop_index,
                    op.unique,
                ),
                op.hmac.as_ref(),
            ),
            BatchOp::DropUser(op) => (canon::canonical_drop_user(&op.drop_user), op.hmac.as_ref()),
            BatchOp::DropRole(op) => (canon::canonical_drop_role(&op.drop_role), op.hmac.as_ref()),
            BatchOp::StartMigration(op) => (
                canon::canonical_start_migration(
                    db_name,
                    &op.repo,
                    &op.start_migration,
                    &op.dst_repo,
                    &op.dst_engine,
                ),
                op.hmac.as_ref(),
            ),
            BatchOp::CommitMigration(op) => (
                canon::canonical_commit_migration(db_name, &op.commit_migration),
                op.hmac.as_ref(),
            ),
            BatchOp::RollbackMigration(op) => (
                canon::canonical_rollback_migration(db_name, &op.rollback_migration),
                op.hmac.as_ref(),
            ),
            _ => continue, // non-destructive — pass.
        };

        let Some(tag) = supplied else {
            return Err((
                alias.clone(),
                "hmac_required",
                "destructive op missing `hmac` field".to_string(),
            ));
        };
        if !canon::verify_tag_hex(&key(&mut key_opt), &canonical, tag) {
            return Err((
                alias.clone(),
                "hmac_mismatch",
                "destructive op `hmac` does not match canonical input".to_string(),
            ));
        }
    }
    Ok(())
}

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
        BatchError::CrossRepoNotSupported { .. } => "tx_cross_repo_not_supported",
    }
}

/// Bridge an async future to a sync caller — context-agnostic.
///
/// # Why not `block_in_place + Handle::block_on`?
///
/// The previous implementation parked the **current Tokio worker thread**
/// for the whole batch (HIGH-7). Combined with `max_active_connections =
/// 10_000` and `max_execution_time_secs = 60` an authenticated peer
/// could saturate the worker pool (default = #CPU cores) from a fraction
/// of those connections by issuing slow batches, denying service to
/// everyone else.
///
/// The new shape:
///   1. Spawn `fut` onto the runtime via `Handle::spawn` — the future
///      runs on a worker thread that is *free to yield* to other tasks
///      while `db.execute` awaits internal I/O.
///   2. The calling thread blocks on a `std::sync::mpsc` channel until
///      the spawned task sends its result back. We use `std::sync::mpsc`
///      instead of `tokio::sync::oneshot::blocking_recv` because the
///      latter goes through `try_enter_blocking_region`, which panics
///      from a runtime worker that has not been marked via
///      `block_in_place` first — and `block_in_place` itself panics
///      from `spawn_blocking` threads. The std primitive is OS-level
///      and works in every context.
///
/// # Threading model
///
/// `connection::request_loop` invokes the sync handler chain inside
/// [`tokio::task::spawn_blocking`] so this function is normally called
/// from a **blocking-pool** thread (default cap 512). Tests that drive
/// the handler directly from a `#[tokio::test(flavor = "multi_thread")]`
/// task call it from a **worker** thread instead. Both paths are
/// correct:
///
///   * Blocking pool → parking that thread is exactly what the pool
///     exists for; runtime workers stay free.
///   * Worker → the spawned future runs on a *different* worker; this
///     thread parks on `recv()`, but the multi-thread runtime in tests
///     (`worker_threads >= 2`) and production has spare workers, so the
///     spawned task makes forward progress independently.
fn run_blocking<F>(fut: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = tokio::runtime::Handle::current();
    let (tx, rx) = std::sync::mpsc::sync_channel::<F::Output>(1);
    handle.spawn(async move {
        let out = fut.await;
        // If the receiver was dropped (caller panicked between spawn and
        // recv) we silently discard the result.
        let _ = tx.send(out);
    });
    rx.recv()
        .expect("run_blocking: spawned task dropped its sender — runtime shut down mid-call")
}
