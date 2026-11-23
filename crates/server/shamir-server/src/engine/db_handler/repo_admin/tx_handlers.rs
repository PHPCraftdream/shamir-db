use shamir_connect::server::session::Session;
use shamir_db::query::batch::{BatchError, BatchOp, BatchRequest};
use shamir_query_types::batch::{BatchTreeVisitor, IsolationLevel, VisibilityWait};
use std::sync::Arc;

use crate::tx_registry::{InteractiveTx, TxRegistryError};

use super::activity::server_busy_response;
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
    ///
    /// R4b-1 — the `tx_open` unit of work is registered in the in-flight
    /// activity table (RAII guard) after every cheap fast-reject gate and
    /// BEFORE the engine open wait (`db.tx_begin_as`), so a BEGIN stuck
    /// waiting on the engine stays visible as `Running`; `client_rid` is the
    /// OPTIONAL transport request id carried alongside the minted
    /// [`RequestExecId`].
    pub(super) async fn tx_begin_with_rid(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        repo_name: &str,
        isolation: Option<IsolationLevel>,
        client_rid: Option<u32>,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }
        // R4b-1 — in-flight registration, one insert + one RAII removal
        // (the guard owns the entry AND the per-session request slot).
        // Per-session cap (R4b-1 follow-up): a stamped session must
        // acquire a slot under `QueryLimitsCap::
        // max_in_flight_requests_per_session` HERE, before the engine
        // open wait; an exhausted session is rejected with the fixed
        // `server_busy` wire text (same code/message as the HTTP REST
        // executor and the serialized launcher overload frame) with no
        // side effect. Skipped ONLY for the stateless (handle-0)
        // fallback. The op-kind is the static discriminant name
        // `tx_open`; no query value ever enters the summary (privacy,
        // D3).
        // Visible in activity; interactive transaction rollback via operator
        // cancellation is deferred to R4b step 3.
        let _in_flight = match self.try_track_activity_request_with_cancellability(
            session, db_name, "tx_open", client_rid, false,
        ) {
            Ok(guard) => guard,
            Err(()) => return server_busy_response(),
        };

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
        match self.tx_registry.register_activity(
            handle,
            it,
            session.activity_handle(),
            Arc::from(session.username.as_str()),
        ) {
            // On reject the just-opened tx (`it`) drops = RAII rollback.
            Err(TxRegistryError::TxAlreadyOpen) => DbResponse::Error {
                code: "tx_already_open".into(),
                message: "session already has an open transaction".into(),
            },
            // R4b-1: tx ids are minted per repo; a collision with a tx from
            // another repo fails closed (the just-opened tx drops = rollback).
            Err(TxRegistryError::TxHandleCollision) => DbResponse::Error {
                code: "tx_handle_conflict".into(),
                message: "transaction handle already in use; retry (per-repo tx id collision)"
                    .into(),
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

    /// Legacy test entry point: the pre-R4b-1 5-argument `tx_begin` (no
    /// transport request id). Test-only forwarding — identical semantics to
    /// `tx_begin_with_rid(..., None)`; the production caller (`handler.rs`)
    /// passes the transport `rid` via `tx_begin_with_rid`.
    #[cfg(test)]
    pub(super) async fn tx_begin(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        repo_name: &str,
        isolation: Option<IsolationLevel>,
    ) -> DbResponse {
        self.tx_begin_with_rid(session, query_version, db_name, repo_name, isolation, None)
            .await
    }

    /// Phase B EXECUTE — run one batch inside an open interactive tx (no
    /// commit). Applies the same per-batch gates as [`Self::execute`]
    /// (version, limits cap, admin, destructive-HMAC), then threads the
    /// parked `TxContext` through the engine glue.
    ///
    /// R4b-1 — the batch is registered in the in-flight activity table
    /// (RAII guard) after every cheap fast-reject gate and BEFORE the
    /// registry acquire and the ctx-lock wait, so an execute stuck waiting
    /// for the parked context stays visible as `Running`. The op-kind is
    /// the static discriminant summary of the batch (homogeneous batches
    /// keep their `activity_name`; heterogeneous ones are conservatively
    /// `write`), derived in the same pre-flight visitor pass that rejects
    /// `ListActivity`, `CancelRequest`, `CancelTx` and `CancelCursorAdmin` —
    /// never from arguments (privacy, D3).
    pub(super) async fn tx_execute_with_rid(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        tx_handle: u64,
        batch: BatchRequest,
        client_rid: Option<u32>,
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

        // R4b-1 — a listing is never transactional. Reject EVERY form
        // (nested in a Batch/ForEach body, mixed with other ops, or
        // standalone) here, BEFORE admission and before the ctx lock is
        // taken, mirroring the `execute` pre-flight. The snapshot of an
        // interactive tx must never serve an operator view that is
        // explicitly nontransactional. The SAME walk derives the batch's
        // op-kind summary (static discriminant names only) AND detects a
        // nested visibility_wait (folded walk, R4b-1 follow-up);
        // precedence is preserved — an activity control op anywhere in the tree
        // is rejected DURING the walk, before the post-walk
        // visibility_wait check runs.
        let mut seen_op: Option<&'static str> = None;
        let mut uniform_ops = true;
        let mut nested_visibility_wait = false;
        for visit in BatchTreeVisitor::new(&batch.queries) {
            let control_op = match &visit.entry.op {
                BatchOp::ListActivity(_) => Some("ListActivity"),
                BatchOp::CancelRequest(_) => Some("CancelRequest"),
                BatchOp::CancelTx(_) => Some("CancelTx"),
                BatchOp::CancelCursorAdmin(_) => Some("CancelCursorAdmin"),
                _ => None,
            };
            if let Some(op) = control_op {
                return DbResponse::Error {
                    code: "invalid_request".into(),
                    message: format!("{op} is not allowed inside a transactional batch"),
                };
            }
            let name = visit.entry.op.activity_name();
            match seen_op {
                None => seen_op = Some(name),
                Some(prev) if prev != name => uniform_ops = false,
                _ => {}
            }
            match &visit.entry.op {
                shamir_query_types::batch::BatchOp::Batch(sub) => {
                    nested_visibility_wait |= sub.batch.visibility_wait.is_some();
                }
                shamir_query_types::batch::BatchOp::ForEach(fe) => {
                    nested_visibility_wait |= fe.batch.visibility_wait.is_some();
                }
                // Only container operations carry nested BatchRequest options.
                _ => {}
            }
        }
        let tx_batch_op_kind = match (seen_op, uniform_ops) {
            (Some(name), true) => name,
            _ => "write",
        };
        if batch.visibility_wait.is_some() || nested_visibility_wait {
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

        // R4b-1 — in-flight registration, one insert + one RAII removal
        // (the guard owns the entry AND the per-session request slot),
        // after every cheap gate (visibility, admin/auth, HMAC) and
        // before the registry acquire / ctx-lock waits. The per-session
        // cap (see `tx_begin_with_rid`) rejects an exhausted session with
        // the fixed `server_busy` text before any execution.
        // Visible in activity; interactive transaction rollback via operator
        // cancellation is deferred to R4b step 3.
        let _in_flight = match self.try_track_activity_request_with_cancellability(
            session,
            db_name,
            tx_batch_op_kind,
            client_rid,
            false,
        ) {
            Ok(guard) => guard,
            Err(()) => return server_busy_response(),
        };

        // Identity-checked stale-failure removals below: `remove_owned`
        // refuses when the reaper already replaced the tx under the same
        // per-repo id.

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
            self.tx_registry.remove_owned(tx_handle, &it);
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
                // The context is gone: an operator cancel (or a commit that
                // already took it) won the race. Same typed answer as a fresh
                // lookup of the removed handle.
                None => {
                    return DbResponse::Error {
                        code: "tx_not_found".into(),
                        message: format!("no open transaction for handle {}", tx_handle),
                    }
                }
            }
        };
        match exec {
            Ok((response, staged_bytes)) => {
                if staged_bytes > self.tx_limits.max_tx_bytes {
                    // Abort: drop the handle (RAII rollback of TxContext +
                    // SnapshotGuard via the Arc) and surface the cap to the
                    // client.
                    self.tx_registry.remove_owned(tx_handle, &it);
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
                    self.tx_registry.remove_owned(tx_handle, &it);
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

        // Commit observation guard (R4b-1): the handle leaves `open` here —
        // no concurrent TxExecute, re-register or reap can find it, so the tx
        // is no longer executable — but the activity-mirror row stays visible
        // as `AwaitingCommit` until `commit_guard` drops, i.e. until the
        // commit pipeline returned (success or failure). The guard holds no
        // ctx lock and never makes the tx executable. `None` = raced the
        // reaper between `get_owned` and now.
        let _commit_guard = match self.tx_registry.begin_commit(tx_handle, &it) {
            Some(guard) => guard,
            None => {
                return DbResponse::Error {
                    code: "tx_not_found".into(),
                    message: format!("no open transaction for handle {}", tx_handle),
                }
            }
        };

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
        // Identity-checked removal (R4b-1): if the reaper already took the
        // tx and another one was re-registered under the same per-repo id,
        // the fresh transaction survives (this rollback drops only OUR
        // parked tx). `None` = raced; the response is unchanged.
        self.tx_registry.remove_owned(tx_handle, &it);
        // Last ref drops here → InteractiveTx drops → TxContext drops (RAII
        // rollback) and the SnapshotGuard releases the GC pin.
        drop(it);
        DbResponse::TxRolledBack { tx_handle }
    }
}

#[cfg(test)]
impl ShamirDbHandler {
    /// Legacy test entry points: the pre-R4b-1 signatures without a
    /// transport request id. Test-only forwarding — identical semantics to
    /// the `*_with_rid(..., None)` forms; the production caller
    /// (`handler.rs`) passes the transport `rid` through the `_with_rid`
    /// methods.
    pub(super) async fn tx_execute(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        tx_handle: u64,
        batch: BatchRequest,
    ) -> DbResponse {
        self.tx_execute_with_rid(session, query_version, db_name, tx_handle, batch, None)
            .await
    }
}
