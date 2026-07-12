use shamir_connect::server::session::Session;
use shamir_db::query::batch::{BatchError, BatchRequest};

use crate::tx_registry::{InteractiveTx, TxRegistryError};

use super::admin::{check_destructive_hmacs, is_coarse_admin_gate_exempt};
use super::handler::{DbResponse, ShamirDbHandler, INTERACTIVE_TX_MAX_LIFETIME};

impl ShamirDbHandler {
    /// Phase B BEGIN — open an interactive tx, park it in the registry bound
    /// to this session, reply with the minted handle + snapshot version.
    pub(super) async fn tx_begin(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        repo_name: &str,
        isolation: Option<String>,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }
        let iso = isolation.unwrap_or_else(|| "snapshot".to_string());

        let actor = super::handler::session_actor(session);
        let begin = self.db.tx_begin_as(actor, db_name, repo_name, &iso).await;
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
                isolation: iso,
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
        mut batch: BatchRequest,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }

        // Server-side cap on per-batch resources (mirror `execute`).
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

        // Destructive-op HMAC gate (same "did you mean it" guard as `execute`).
        if let Err((alias, code, message)) = check_destructive_hmacs(session, db_name, &batch) {
            return DbResponse::Error {
                code: code.into(),
                message: format!("query '{}': {}", alias, message),
            };
        }

        // Look up the handle and verify it belongs to this session.
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
                    let r = self.db.tx_execute_as(actor, db_name, &batch, tx).await;
                    if r.is_ok() {
                        it.bump_activity();
                    }
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
            Err(e) => DbResponse::Error {
                code: super::handler::error_code(&e).to_string(),
                message: e.to_string(),
            },
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

        // Remove first: no concurrent TxExecute can find the handle once we
        // start committing.
        self.tx_registry.remove(tx_handle);

        let repo = it.repo().to_string();
        let actor = super::handler::session_actor(session);
        let tx = it.ctx().lock().await.take();
        let commit = match tx {
            Some(tx) => self.db.tx_commit_as(actor, db_name, &repo, tx).await,
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
