//! `execute` — the admitted-batch execution path of [`ShamirDbHandler`]
//! (moved verbatim out of `handler.rs` by the R4b-1 narrow recovery so the
//! bridge file stays under the 1000-line limit; identical visibility
//! (`pub(super)` of the `db_handler` module — every caller lives in that
//! subtree), same behaviour, no new re-exports).
//!
//! R4b-1 in this file:
//! - the batch is registered in the in-flight activity table (RAII) after
//!   every cheap fast-reject gate and BEFORE the RI-15 byte-budget wait;
//! - the op-kind summary is derived during the SAME preflight walk that
//!   resolves nested visibility waits and validates `ListActivity` form —
//!   static discriminant names only: a homogeneous batch keeps its exact
//!   `activity_name` (`read` / `write` / `admin:<op>`), a heterogeneous
//!   batch is conservatively `write`; no generic `batch` string, and no
//!   query value ever enters the summary;
//! - a single standalone `ListActivity` batch routes to the paging worker
//!   (`activity::list`) through the SHARED byte-budget path — the
//!   requested `max_result_size` (and the server cap) still bound the
//!   page, and the same shrink/stash tail applies to the page response as
//!   to a normal batch response;
//! - the existing slow-query warning additionally carries the minted
//!   [`RequestExecId`] and the OPTIONAL client `rid` (ids only — never the
//!   bearer token or its prefix, no new log lines).

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;

use shamir_db::query::batch::{BatchOp, BatchRequest};
use shamir_query_types::admin::activity::ListActivityReq;
use shamir_query_types::batch::BatchTreeVisitor;
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
    /// [`ShamirDb::execute`] on the current Tokio worker.
    ///
    /// R4b-1 — the batch is registered in the in-flight activity table
    /// (RAII) after every cheap fast-reject gate and BEFORE the RI-15
    /// byte-budget wait, so an operator can see a request stuck waiting
    /// for budget; `client_rid` is the OPTIONAL transport request id
    /// carried alongside the minted [`RequestExecId`].
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

        // R4b-1 — form validation for the `ListActivity` operation. A
        // listing is ONLY valid as a standalone, nontransactional
        // single-op batch; every other form is rejected here, in the same
        // single pre-execution visitor pass that resolves nested
        // visibility waits AND derives the batch's op-kind summary —
        // BEFORE admission, the budget wait and any writes, and without
        // scanning the tree a second time.
        enum PreFlightError {
            Db(shamir_db::DbError),
            ActivityForm(&'static str),
        }
        let mut single_activity: Option<ListActivityReq> = None;
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
            let mut activity_leaves: usize = 0;
            for visit in BatchTreeVisitor::new(&batch.queries) {
                // R4b-1 — classification rides the same walk: static
                // discriminant names only.
                let name = visit.entry.op.activity_name();
                match seen_op {
                    None => seen_op = Some(name),
                    Some(prev) if prev != name => uniform_ops = false,
                    _ => {}
                }
                if let shamir_query_types::batch::BatchOp::ListActivity(req) = &visit.entry.op {
                    if visit.body_depth().is_some() {
                        return Err(PreFlightError::ActivityForm(
                            "ListActivity cannot be nested inside a Batch/ForEach body",
                        ));
                    }
                    activity_leaves += 1;
                    single_activity = Some(req.clone());
                }
                let nested = match &visit.entry.op {
                    shamir_query_types::batch::BatchOp::Batch(sub) => sub.batch.visibility_wait,
                    shamir_query_types::batch::BatchOp::ForEach(fe) => fe.batch.visibility_wait,
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
            if activity_leaves > 0 {
                if batch.transactional {
                    return Err(PreFlightError::ActivityForm(
                        "ListActivity is not allowed inside a transactional batch",
                    ));
                }
                if batch.queries.len() != 1 || activity_leaves != 1 {
                    return Err(PreFlightError::ActivityForm(
                        "ListActivity must be the ONLY operation of its batch",
                    ));
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
            Err(PreFlightError::ActivityForm(reason)) => {
                return DbResponse::Error {
                    code: "invalid_request".into(),
                    message: reason.to_string(),
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

        // R4b-1 — in-flight registration. Placed AFTER every cheap gate
        // (version, form, admin, read-only, HMAC) and BEFORE the RI-15
        // byte-budget acquire: a request stuck waiting for budget stays
        // visible as `Running` to the operator. One insert; the RAII guard
        // below removes the entry, decrements the mirror and releases the
        // per-session request slot exactly once on EVERY exit path (panic
        // included). Per-session cap (R4b-1 follow-up): a stamped session
        // must acquire a slot under `QueryLimitsCap::
        // max_in_flight_requests_per_session` HERE, before execution and
        // the budget wait; an exhausted session is rejected with the fixed
        // `server_busy` wire text (the same code/message the HTTP REST
        // executor and the serialized launcher overload frame already
        // emit) BEFORE any execution, so a real write is refused with no
        // side effect. The cap is skipped ONLY for the stateless
        // (handle-0) fallback. `op_kind` is the STATIC discriminant
        // summary derived during the preflight walk.
        let op_kind = match (seen_op, uniform_ops) {
            (Some(name), true) => name,
            _ => "write",
        };
        let _in_flight =
            match self.try_track_activity_request(session, db_name, op_kind, client_rid) {
                Ok(guard) => guard,
                Err(()) => return server_busy_response(),
            };
        let exec_id = _in_flight.id();

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
        // above (version check, form, admin gate, read-only gate, HMAC
        // gate) and AFTER the in-flight registration — those should stay
        // free of any budget interaction. The guard is held locally for
        // the rest of this function and shrunk to the ACTUAL serialized
        // size once the final response is known (see the single tail
        // below), rather than stashed immediately: stashing happens
        // exactly once, at the end, same as before this change.
        //
        // R4b-1 follow-up — the SINGLE-op `ListActivity` route makes NO
        // upfront reservation: a listing is an operator control-plane
        // read and must not reserve (let alone park behind) the global
        // response-byte budget — a separate session can list while the
        // budget is fully saturated by data workloads, and no arbitrary
        // memory is reserved for a page whose size is already bounded by
        // the cap below and the page `limit`. The COMMON serialized
        // byte/result limit below (the `result_too_large` check against
        // min(request `max_result_size`, server cap)) is still enforced
        // for the page like for any other response.
        let reserve_bytes = batch
            .limits
            .max_result_size
            .min(self.query_limits.max_result_size_bytes);
        let mut budget_guard = if single_activity.is_none() && self.byte_budget.cap().is_some() {
            Some(self.byte_budget.acquire(reserve_bytes).await)
        } else {
            None
        };

        let final_response = if let Some(req) = single_activity {
            // R4b-1 — single-op `ListActivity` routing. The batch already
            // passed every gate above and is registered in-flight; route
            // to the paging worker (`activity::list`) and let the guard's
            // drop mark it Done + remove it. The worker receives
            // `Some(exec_id)` so it can EXCLUDE this listing request
            // itself from the returned `Requests` rows (otherwise every
            // page could mint a fresh row forever).
            self.list_activity(session, db_name, &req, Some(exec_id))
                .await
        } else {
            let actor = super::handler::session_actor(session);
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
                            client_rid = ?_in_flight.client_rid(),
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
        };

        // RI-15 (CR-B2) — single convergence point for BOTH the success and
        // error paths (and, R4b-1, the `ListActivity` page path): serialize
        // the final `DbResponse` envelope exactly ONCE, shrink the upfront
        // reservation down to that actual size (an error response's tiny
        // actual size still needs its pessimistic over-reservation narrowed
        // back down), then stash both the guard and the already-serialized
        // bytes so `RequestHandler::handle` reuses them instead of encoding
        // the SAME response a second time.
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
