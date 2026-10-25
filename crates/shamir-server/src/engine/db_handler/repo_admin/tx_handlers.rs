use shamir_connect::server::session::Session;
use shamir_db::query::batch::{BatchError, BatchRequest};
use shamir_query_types::batch::{BatchTreeVisitor, IsolationLevel, VisibilityWait};

use crate::tx_registry::{InteractiveTx, TxRegistryError};

use super::admin::{check_destructive_hmacs, is_coarse_admin_gate_exempt};
use super::handler::{DbResponse, ShamirDbHandler, INTERACTIVE_TX_MAX_LIFETIME};

use crate::db_handler::config::NodeMode;

/// RV-SERVER-C73466A7: `execute`'s read-only replica gate applied to the
/// interactive path (same contract, same classifier). A `ReadOnly` node
/// still SERVES reads inside transactions — the card explicitly preserves
/// permitted reads (P0.md, "сохранении actor DAC и разрешённых reads") —
/// so a tx batch is rejected with the existing `read_only_replica` error
/// only when it contains a mutating operation, classified by the SAME
/// `BatchOp::is_write` visitor the non-transactional `execute` uses
/// (`handler.rs`). No-op for `ReadWrite`.
fn read_only_tx_write_gate(node_mode: NodeMode, batch: &BatchRequest) -> Option<DbResponse> {
    if node_mode != NodeMode::ReadOnly {
        return None;
    }
    for visit in BatchTreeVisitor::new(&batch.queries).filter(|visit| visit.body_depth().is_none())
    {
        let (alias, entry) = (visit.alias, visit.entry);
        if entry.op.is_write() {
            return Some(DbResponse::Error {
                code: "read_only_replica".into(),
                message: format!(
                    "query '{}' is a write; this node is a read-only replica",
                    alias
                ),
            });
        }
    }
    None
}

impl ShamirDbHandler {
    /// Phase B BEGIN — open an interactive tx, park it in the registry bound
    /// to this session, reply with the minted handle + snapshot version.
    pub(super) async fn tx_begin(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        repo_name: &str,
        isolation: Option<IsolationLevel>,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }
        // RV-SERVER-C73466A7: TxBegin is NOT mode-gated — opening a
        // snapshot transaction is read-capable and a read-only replica must
        // keep serving reads inside interactive transactions. Writes are
        // refused per-op at TxExecute and at commit (below).
        let iso = isolation.unwrap_or(IsolationLevel::Snapshot);

        let actor = super::handler::session_actor(session);
        // `ShamirDb::tx_begin_as` still speaks the wire-string vocabulary;
        // `IsolationLevel::as_str` is exactly that vocabulary (v4 enum).
        let begin = self
            .db
            .tx_begin_as(actor, db_name, repo_name, iso.as_str())
            .await;
        let (tx, guard) = match begin {
            Ok(pair) => pair,
            Err(e) => {
                return DbResponse::Error {
                    code: super::handler::error_code(&e).to_string(),
                    message: e.to_string(),
                }
            }
        };

        let handle = tx.tx_id.0;
        let snapshot_version = tx.snapshot_version;
        let it = InteractiveTx::new(
            tx,
            guard,
            session.session_id,
            session.user_id,
            db_name.to_string(),
            repo_name.to_string(),
            INTERACTIVE_TX_MAX_LIFETIME,
        );
        match self.tx_registry.register(handle, it) {
            // On reject the just-opened tx (`it`) drops = RAII rollback.
            Err(TxRegistryError::TxAlreadyOpen) => DbResponse::Error {
                code: "tx_already_open".into(),
                message: "session already has an open transaction".into(),
            },
            Err(_) => DbResponse::Error {
                code: "tx_error".into(),
                message: "could not register transaction".into(),
            },
            Ok(_) => DbResponse::TxOpened {
                tx_handle: handle,
                snapshot_version,
                isolation: iso.as_str().to_string(),
            },
        }
    }

    /// Phase B EXECUTE — run one batch inside an open interactive tx (no
    /// commit). Applies the same per-batch gates as [`Self::execute`]
    /// (version, limits cap, admin, destructive-HMAC), then threads the
    /// parked `TxContext` through the engine glue.
    pub(super) async fn tx_execute(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        tx_handle: u64,
        batch: BatchRequest,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }

        if let Some(resp) = read_only_tx_write_gate(self.node_mode, &batch) {
            return resp;
        }

        let nested_override =
            BatchTreeVisitor::new(&batch.queries).any(|visit| match &visit.entry.op {
                shamir_query_types::batch::BatchOp::Batch(sub) => {
                    sub.batch.visibility_wait.is_some()
                }
                shamir_query_types::batch::BatchOp::ForEach(fe) => {
                    fe.batch.visibility_wait.is_some()
                }
                // Only container operations carry nested BatchRequest options.
                _ => false,
            });
        if batch.visibility_wait.is_some() || nested_override {
            return DbResponse::Error {
                code: "validation".into(),
                message: "visibility_wait is commit-only; use TxCommit".into(),
            };
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

        // Destructive-op HMAC gate (same "did you mean it" guard as `execute`).
        if let Err((alias, code, message)) = check_destructive_hmacs(session, db_name, &batch) {
            return DbResponse::Error {
                code: code.into(),
                message: format!("query '{}': {}", alias, message),
            };
        }

        // Look up the handle and verify it belongs to this session.
        let (it, _lease) = match self
            .tx_registry
            .acquire_owned(tx_handle, &session.session_id)
        {
            Ok(acquired) => acquired,
            Err(TxRegistryError::TxNotFound) => {
                return DbResponse::Error {
                    code: "tx_not_found".into(),
                    message: format!("no open transaction for handle {}", tx_handle),
                }
            }
            Err(_) => {
                return DbResponse::Error {
                    code: "tx_forbidden".into(),
                    message: "transaction handle does not belong to this session".into(),
                }
            }
        };

        if it.remaining_lifetime().is_zero() {
            self.tx_registry.remove(tx_handle);
            drop(it);
            return DbResponse::Error {
                code: "tx_expired".into(),
                message: "interactive transaction exceeded its absolute lifetime".into(),
            };
        }
        // Preserve wire limits: the exact remaining lifetime is passed below,
        // after acquiring the context lock, so lock contention cannot renew it.

        // The handle is pinned to one (db, repo); every TxExecute must match.
        if it.db() != db_name {
            return DbResponse::Error {
                code: "tx_wrong_db".into(),
                message: format!("handle pinned to db '{}', got '{}'", it.db(), db_name),
            };
        }
        for r in shamir_query_types::batch::distinct_repos(&batch.queries) {
            if r != it.repo() {
                return DbResponse::Error {
                    code: "tx_wrong_repo".into(),
                    message: format!(
                        "TxExecute targets repo '{}' but the handle is pinned to '{}'",
                        r,
                        it.repo()
                    ),
                };
            }
        }

        let actor = super::handler::session_actor(session);
        let exec = {
            let mut guard = it.ctx().lock().await;
            match guard.as_mut() {
                Some(tx) => {
                    let repo_observer = self.audit.as_ref().map(|audit| {
                        super::repo_audit::repo_audit_observer(audit.clone(), session)
                    });
                    let r = self
                        .db
                        .tx_execute_with_policy_and_budget_and_repo_observer(
                            actor,
                            db_name,
                            &batch,
                            tx,
                            self.execution_policy(),
                            Some(it.remaining_lifetime()),
                            repo_observer,
                        )
                        .await;
                    // Measure staged size INSIDE the same lock as the engine call.
                    r.map(|resp| (resp, guard.as_ref().map_or(0, |tx| tx.staged_bytes())))
                }
                None => Err(BatchError::QueryError {
                    alias: String::new(),
                    message: "transaction is already committed or rolled back".into(),
                    code: None,
                }),
            }
        };
        match exec {
            Ok((response, staged_bytes)) => {
                if staged_bytes > self.tx_limits.max_tx_bytes {
                    // Abort: drop the handle (RAII rollback of TxContext +
                    // SnapshotGuard via the Arc) and surface the cap to the
                    // client.
                    self.tx_registry.remove(tx_handle);
                    drop(it);
                    return DbResponse::Error {
                        code: "tx_too_large".into(),
                        message: format!(
                            "interactive transaction exceeded max_tx_bytes ({} > {})",
                            staged_bytes, self.tx_limits.max_tx_bytes
                        ),
                    };
                }
                DbResponse::TxBatch { response }
            }
            Err(e) => {
                if matches!(
                    &e,
                    BatchError::ExecutionTimedOut { .. } | BatchError::LimitExceeded { .. }
                ) {
                    self.tx_registry.remove(tx_handle);
                    drop(it);
                }
                super::cursor_handlers::error_response(&e)
            }
        }
    }

    /// Phase B COMMIT — remove the handle from the registry, run the Phase-A
    /// commit pipeline on its `TxContext`, reply with the `TransactionInfo`.
    /// The owning `Arc` (and thus the `SnapshotGuard`) is held alive until
    /// commit returns, so the MVCC snapshot stays pinned through commit.
    pub(super) async fn tx_commit(
        &self,
        session: &Session,
        db_name: &str,
        tx_handle: u64,
    ) -> DbResponse {
        self.tx_commit_with_options(session, db_name, tx_handle, None)
            .await
    }

    pub(super) async fn tx_commit_with_options(
        &self,
        session: &Session,
        db_name: &str,
        tx_handle: u64,
        wait: Option<VisibilityWait>,
    ) -> DbResponse {
        let it = match self.tx_registry.get_owned(tx_handle, &session.session_id) {
            Ok(it) => it,
            Err(TxRegistryError::TxNotFound) => {
                return DbResponse::Error {
                    code: "tx_not_found".into(),
                    message: format!("no open transaction for handle {}", tx_handle),
                }
            }
            Err(_) => {
                return DbResponse::Error {
                    code: "tx_forbidden".into(),
                    message: "transaction handle does not belong to this session".into(),
                }
            }
        };
        if it.db() != db_name {
            return DbResponse::Error {
                code: "tx_wrong_db".into(),
                message: format!("handle pinned to db '{}', got '{}'", it.db(), db_name),
            };
        }

        // RV-SERVER-C73466A7: a read-only node must never land staged
        // writes. The commit is refused ONLY when the tx actually staged a
        // write; a read-only (snapshot) tx commits normally.
        let staged_bytes = {
            let guard = it.ctx().lock().await;
            guard.as_ref().map_or(0, |tx| tx.staged_bytes())
        };
        if self.node_mode == NodeMode::ReadOnly && staged_bytes > 0 {
            return DbResponse::Error {
                code: "read_only_replica".into(),
                message: format!(
                    "transaction has staged writes ({} bytes); this node is a read-only replica",
                    staged_bytes
                ),
            };
        }

        // Remove first: no concurrent TxExecute can find the handle once we
        // start committing.
        self.tx_registry.remove(tx_handle);

        // Invalid commit policy consumes a correctly addressed owned handle too.
        // Dropping `it` releases the snapshot and rolls back staged writes.
        let wait = match self.transactions.resolve_commit_wait(wait) {
            Ok(wait) => wait,
            Err(e) => {
                drop(it);
                return DbResponse::Error {
                    code: "visibility_wait_invalid".into(),
                    message: e.to_string(),
                };
            }
        };
        let repo = it.repo().to_string();
        let actor = super::handler::session_actor(session);
        let tx = it.ctx().lock().await.take();
        let commit = match tx {
            Some(tx) => {
                self.db
                    .tx_commit_as_with_options(actor, db_name, &repo, tx, wait)
                    .await
            }
            None => Err(BatchError::QueryError {
                alias: String::new(),
                message: "transaction is already committed or rolled back".into(),
                code: None,
            }),
        };
        // `it` (holding the SnapshotGuard) drops here, AFTER commit returned —
        // the snapshot stayed pinned through commit.
        drop(it);
        match commit {
            Ok(transaction) => DbResponse::TxCommitted { transaction },
            Err(e) => DbResponse::Error {
                code: super::handler::error_code(&e).to_string(),
                message: e.to_string(),
            },
        }
    }

    /// Phase B ROLLBACK — remove the handle and drop the parked tx (RAII
    /// rollback, no storage side effects).
    pub(super) async fn tx_rollback(
        &self,
        session: &Session,
        db_name: &str,
        tx_handle: u64,
    ) -> DbResponse {
        let it = match self.tx_registry.get_owned(tx_handle, &session.session_id) {
            Ok(it) => it,
            Err(TxRegistryError::TxNotFound) => {
                return DbResponse::Error {
                    code: "tx_not_found".into(),
                    message: format!("no open transaction for handle {}", tx_handle),
                }
            }
            Err(_) => {
                return DbResponse::Error {
                    code: "tx_forbidden".into(),
                    message: "transaction handle does not belong to this session".into(),
                }
            }
        };
        if it.db() != db_name {
            return DbResponse::Error {
                code: "tx_wrong_db".into(),
                message: format!("handle pinned to db '{}', got '{}'", it.db(), db_name),
            };
        }
        self.tx_registry.remove(tx_handle);
        // Last ref drops here → InteractiveTx drops → TxContext drops (RAII
        // rollback) and the SnapshotGuard releases the GC pin.
        drop(it);
        DbResponse::TxRolledBack { tx_handle }
    }
}
