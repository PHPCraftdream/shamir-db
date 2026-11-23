//! Admitted batch dispatch, activity tracking, and response budgeting.

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;

use shamir_db::query::batch::{BatchError, BatchOp, BatchRequest};
use shamir_query_types::admin::activity::{ListActivityReq, RequestExecId};
use shamir_query_types::admin::shape_stats::{ListShapeStatsReq, ResetShapeStatsReq};
use shamir_query_types::batch::BatchTreeVisitor;
use shamir_query_types::wire::CursorId;
use shamir_query_types::wire::DbResponse;

use super::activity::server_busy_response;
use super::admin::{check_destructive_hmacs, is_coarse_admin_gate_exempt};
use super::config::NodeMode;
use super::cursor_handlers::error_response;
use super::handlers::backup_handlers;
use super::repo_audit;
use super::subscribe_handler;
use super::ShamirDbHandler;

impl ShamirDbHandler {
    /// Run the version check + admin gate, then forward to
    /// the database executor on the current Tokio worker.
    ///
    /// R4b-1 — the batch is registered in the in-flight activity table
    /// (RAII) after every cheap fast-reject gate and BEFORE the RI-15
    /// byte-budget wait, so an operator can see a request stuck waiting
    /// for budget; `client_rid` is the OPTIONAL transport request id
    /// carried alongside the minted [`RequestExecId`].
    ///
    /// The three cancel operations (`CancelRequest`, `CancelTx`,
    /// `CancelCursorAdmin`) bypass admission entirely, so a cancel reaches
    /// saturated work (or a registry row) even when its session has no
    /// in-flight slot left.
    ///
    /// cancel-safe: NO — drive this future to completion once DB execution starts.
    pub(super) async fn execute_with_rid(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        mut batch: BatchRequest,
        conn: &ConnectionServices,
        client_rid: Option<u32>,
    ) -> DbResponse {
        // Query-language version dispatch — fast reject before any DB work.
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }

        // A control op must be standalone and nontransactional. Validate its
        // form during the same walk as visibility waits and op-kind summary.
        enum PreFlightError {
            Db(shamir_db::DbError),
            ActivityForm(&'static str, &'static str),
        }
        let mut single_activity: Option<ListActivityReq> = None;
        let mut single_cancel: Option<RequestExecId> = None;
        let mut single_cancel_tx: Option<u64> = None;
        let mut single_cancel_cursor: Option<CursorId> = None;
        // R4b step 4 — the shape-statistics control ops ride the same
        // standalone, non-transactional form rules as the activity listing
        // and the cancels.
        let mut single_list_shapes: Option<ListShapeStatsReq> = None;
        let mut single_reset_shapes: Option<ResetShapeStatsReq> = None;
        // R4b-1 — classification rides the same walk: static
        // discriminant names only. Vocabulary: a homogeneous batch keeps
        // its exact `activity_name` (`read` / `write` / `admin:<op>`);
        // a heterogeneous batch is conservatively `write` (it may
        // mutate); there is no generic `batch` string and no query value
        // ever enters the summary.
        let mut seen_op: Option<&'static str> = None;
        let mut uniform_ops = true;
        let policy = (|| {
            let wait = self
                .transactions
                .resolve_commit_wait(batch.visibility_wait)
                .map_err(PreFlightError::Db)?;
            for visit in BatchTreeVisitor::new(&batch.queries) {
                // R4b-1 — classification rides the same walk: static
                // discriminant names only.
                let name = visit.entry.op.activity_name();
                match seen_op {
                    None => seen_op = Some(name),
                    Some(prev) if prev != name => uniform_ops = false,
                    _ => {}
                }
                let op = &visit.entry.op;
                let control_op = match op {
                    BatchOp::ListActivity(_) => Some("ListActivity"),
                    BatchOp::CancelRequest(_) => Some("CancelRequest"),
                    BatchOp::CancelTx(_) => Some("CancelTx"),
                    BatchOp::CancelCursorAdmin(_) => Some("CancelCursorAdmin"),
                    // R4b step 4: a shape listing or wipe is never
                    // transactional and never nested (it addresses
                    // node-local telemetry, not the batch's data view).
                    BatchOp::ListShapeStats(_) => Some("ListShapeStats"),
                    BatchOp::ResetShapeStats(_) => Some("ResetShapeStats"),
                    _ => None,
                };
                if let Some(name) = control_op {
                    let reason = if visit.body_depth().is_some() {
                        Some("cannot be nested inside a Batch/ForEach body")
                    } else if batch.transactional {
                        Some("is not allowed inside a transactional batch")
                    } else if batch.queries.len() != 1 {
                        Some("must be the ONLY operation of its batch")
                    } else if matches!(
                        op,
                        BatchOp::CancelRequest(_)
                            | BatchOp::CancelTx(_)
                            | BatchOp::CancelCursorAdmin(_)
                            // A reset mutates local registry state, so it
                            // carries the cancels' no-conditions rule; a
                            // listing follows `ListActivity` and may carry
                            // `when`/`after` like any other single op.
                            | BatchOp::ResetShapeStats(_)
                    ) && (visit.entry.when.is_some() || !visit.entry.after.is_empty())
                    {
                        Some("does not support execution conditions or dependencies")
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        return Err(PreFlightError::ActivityForm(name, reason));
                    }
                }
                // Visit container bodies separately.
                let nested = match op {
                    BatchOp::ListActivity(req) => {
                        single_activity = Some(req.clone());
                        None
                    }
                    BatchOp::CancelRequest(id) => {
                        single_cancel = Some(*id);
                        None
                    }
                    BatchOp::CancelTx(handle) => {
                        single_cancel_tx = Some(*handle);
                        None
                    }
                    BatchOp::CancelCursorAdmin(id) => {
                        single_cancel_cursor = Some(*id);
                        None
                    }
                    BatchOp::ListShapeStats(req) => {
                        single_list_shapes = Some(req.clone());
                        None
                    }
                    BatchOp::ResetShapeStats(req) => {
                        single_reset_shapes = Some(req.clone());
                        None
                    }
                    BatchOp::Batch(sub) => sub.batch.visibility_wait,
                    BatchOp::ForEach(fe) => fe.batch.visibility_wait,
                    _ => None,
                };
                if let Some(nested) = nested {
                    self.transactions
                        .resolve_wait(nested)
                        .map_err(PreFlightError::Db)?;
                    if batch.transactional
                        && matches!(
                            nested,
                            shamir_query_types::batch::VisibilityWait::Bounded { .. }
                        )
                    {
                        return Err(PreFlightError::Db(
                            shamir_db::DbError::VisibilityWaitInvalid {
                                reason:
                                    "bounded visibility is unsupported by nested Batch/ForEach bodies"
                                        .into(),
                            },
                        ));
                    }
                }
            }
            Ok(wait)
        })();
        match policy {
            Ok(wait) => batch.visibility_wait = Some(wait),
            Err(PreFlightError::Db(e)) => {
                return DbResponse::Error {
                    code: "visibility_wait_invalid".into(),
                    message: e.to_string(),
                };
            }
            Err(PreFlightError::ActivityForm(name, reason)) => {
                return DbResponse::Error {
                    code: "invalid_request".into(),
                    message: format!("{name} {reason}"),
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
        // R1: include leader_addr for client redirect. R1c (§4.5): a
        // follower has no consistent journal+schema view, so EVERY
        // application-schema arm (status included) is refused first with
        // the stable `schema_migration_follower_read_only` code.
        if self.node_mode == NodeMode::ReadOnly {
            for visit in
                BatchTreeVisitor::new(&batch.queries).filter(|visit| visit.body_depth().is_none())
            {
                let (alias, entry) = (visit.alias, visit.entry);
                if matches!(entry.op, BatchOp::ApplicationSchema(_)) {
                    return DbResponse::Error {
                        code: "schema_migration_follower_read_only".into(),
                        message: format!(
                            "query '{}': application-schema operations are refused on a \
                             follower/read replica (no consistent journal+schema view)",
                            alias
                        ),
                    };
                }
                if entry.op.is_write() {
                    return backup_handlers::follower_write_refusal(&entry.op, alias);
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

        let reserve_bytes = batch
            .limits
            .max_result_size
            .min(self.query_limits.max_result_size_bytes);
        let mut budget_guard = None;
        let control_op =
            single_cancel.is_some() || single_cancel_tx.is_some() || single_cancel_cursor.is_some();
        // Keep the request slot through response encoding on tracked paths.
        let in_flight;
        let final_response = if let Some(id) = single_cancel {
            // Cancel must reach saturated work even if its session has no slots
            // — true for all three cancel operations, which skip admission.
            Box::pin(self.cancel_request(session, id)).await
        } else if let Some(handle) = single_cancel_tx {
            Box::pin(self.cancel_tx(session, handle)).await
        } else if let Some(cursor_id) = single_cancel_cursor {
            Box::pin(self.cancel_cursor_admin(session, cursor_id)).await
        } else {
            let op_kind = match (seen_op, uniform_ops) {
                (Some(name), true) => name,
                _ => "write",
            };
            let registration =
                self.try_track_activity_request(session, db_name, op_kind, client_rid);
            in_flight = match registration {
                Ok(guard) => guard,
                Err(()) => return server_busy_response(),
            };
            let exec_id = in_flight.id();
            let cancel = in_flight.cancel_handle();

            // Listing is tracked but never parks behind the shared byte budget.
            // Other requests register first so operators can see and wake them.
            // R4b step 4: the shape listing and the shape reset follow the
            // same rule — both address node-local telemetry (a bounded
            // in-memory snapshot, or one atomic wipe) and neither may be
            // held behind a saturated data-path budget. Only the cancel
            // family is a `control_op` for the encoded-result cap, because
            // its refusal envelope must reach saturated work; a shape page
            // stays bounded by the caller's own `max_result_size` exactly
            // like `ListActivity`.
            let needs_budget = single_activity.is_none()
                && single_list_shapes.is_none()
                && single_reset_shapes.is_none()
                && self.byte_budget.cap().is_some();
            if needs_budget {
                budget_guard = self
                    .byte_budget
                    .acquire_cancellable(reserve_bytes, cancel)
                    .await;
            }
            if needs_budget && budget_guard.is_none() {
                error_response(&BatchError::Cancelled)
            } else if let Some(req) = single_activity {
                let response = self
                    .list_activity(session, db_name, &req, Some(exec_id))
                    .await;
                if in_flight.finish() {
                    response
                } else {
                    error_response(&BatchError::Cancelled)
                }
            } else if let Some(req) = single_list_shapes {
                // Read-only, local: no node-mode gate (the registry is
                // in-memory per-repository telemetry, `is_write() == false`),
                // exactly like `ListActivity`.
                let response = self.list_shape_stats(session, db_name, &req).await;
                if in_flight.finish() {
                    response
                } else {
                    error_response(&BatchError::Cancelled)
                }
            } else if let Some(req) = single_reset_shapes {
                // One in-memory wipe, gated by the same completion shape as
                // the listing above: the reset claims the commit slot before
                // it touches the registry, so a cancellation that already
                // won is answered `cancelled` and nothing is wiped, while a
                // cancellation arriving after the claim loses to
                // `finish()`. No node-mode gate for the same reason as the
                // listing above.
                let response = self
                    .reset_shape_stats(session, db_name, &req, in_flight.cancel_handle())
                    .await;
                if in_flight.finish() {
                    response
                } else {
                    error_response(&BatchError::Cancelled)
                }
            } else {
                // Always await the DB; dropping it can interrupt a commit.
                let actor = super::handler::session_actor(session);
                let repo_observer = self
                    .audit
                    .as_ref()
                    .map(|audit| repo_audit::repo_audit_observer(audit.clone(), session));
                let exec_result = self
                    .db
                    .execute_with_policy_and_repo_observer_and_cancel(
                        actor.clone(),
                        db_name,
                        &batch,
                        self.execution_policy(),
                        repo_observer,
                        Some(cancel.clone()),
                    )
                    .await;
                if !in_flight.finish() {
                    error_response(&BatchError::Cancelled)
                } else {
                    match exec_result {
                        Ok(mut response) => {
                            // Slow-query logging: WARN line for batches whose total
                            // execution time exceeds the configured threshold. Useful
                            // for spotting unindexed queries in production. Threshold
                            // = 0 disables (e.g. in unit tests). R4b-1: the minted
                            // [`RequestExecId`] and the OPTIONAL client `rid` ride
                            // along (ids only — no token, no prefix, no new lines).
                            if self.slow_query.threshold_us > 0
                                && response.execution_time_us > self.slow_query.threshold_us
                            {
                                tracing::warn!(
                                    elapsed_us = response.execution_time_us,
                                    threshold_us = self.slow_query.threshold_us,
                                    db = db_name,
                                    queries = batch.queries.len(),
                                    request_id = ?response.id,
                                    request_exec_id = exec_id.0,
                                    client_rid = ?in_flight.client_rid(),
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
                            )
                            .await;

                            DbResponse::Batch { response }
                        }
                        Err(e) => error_response(&e),
                    }
                }
            }
        };

        // Like rejection envelopes, the fixed cancellation error is encoded
        // by the handler even when the target's result cap is too small.
        if matches!(
            &final_response,
            DbResponse::Error { code, .. }
                if code == shamir_query_types::wire::error_code::CANCELLED
        ) {
            drop(budget_guard);
            return final_response;
        }

        // RI-15 (CR-B2) — single convergence point for capped success and
        // error replies (including all three cancel operations): serialize
        // the final `DbResponse` envelope exactly ONCE, shrink the upfront
        // reservation down to that actual size (any reply's tiny actual
        // size still needs its pessimistic over-reservation narrowed back
        // down), then stash both the guard and the already-serialized
        // bytes so `RequestHandler::handle` reuses them instead of encoding
        // the SAME response a second time.
        //
        // The two kinds of reply are fenced differently. A DATA reply is
        // capped at `reserve_bytes` — the caller's own `max_result_size`
        // clamped to the operator's limit. An ERROR reply is fenced only by
        // the operator's `max_result_size_bytes`: a refusal is a fixed
        // envelope carrying the reason the caller got nothing, so a caller
        // asking for `max_result_size: 1` must not have `unknown_table`,
        // `server_busy` (or any other coded refusal) rewritten into
        // `result_too_large` — that would destroy the cause. This is the
        // same rule as the `cancelled` carve-out above. The operator's cap
        // still bounds an error envelope whose message embeds
        // caller-supplied text (e.g. a table name).
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
            // A refusal keeps the operator's fence, never the caller's.
            let size_cap = match &final_response {
                DbResponse::Error { .. } => self.query_limits.max_result_size_bytes,
                _ => reserve_bytes,
            };
            if !control_op && bytes.len() > size_cap {
                // Release before returning; never stash the rejected payload.
                drop(budget_guard);
                return DbResponse::Error {
                    code: "result_too_large".to_string(),
                    message: format!(
                        "encoded response is {} bytes; maximum is {size_cap}",
                        bytes.len(),
                    ),
                };
            }
            if let Some(guard) = budget_guard.as_mut() {
                guard.shrink_to(bytes.len());
            }
            if let Some(guard) = budget_guard {
                crate::byte_budget::stash_guard(guard);
            }
            crate::byte_budget::stash_serialized_response(bytes);
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

#[cfg(test)]
impl ShamirDbHandler {
    /// Legacy test entry point: the pre-R4b-1 5-argument `execute`
    /// (no transport request id). Test-only forwarding — identical
    /// semantics to `execute_with_rid(..., None)`; production callers
    /// (`handler.rs`) use `execute_with_rid` directly so the rid-carrying
    /// path stays the single real implementation.
    pub(super) async fn execute(
        &self,
        session: &Session,
        query_version: u32,
        db_name: &str,
        batch: BatchRequest,
        conn: &ConnectionServices,
    ) -> DbResponse {
        self.execute_with_rid(session, query_version, db_name, batch, conn, None)
            .await
    }
}
