//! CancelRequest looks up ownership without taking a registry writer lock.

use shamir_connect::server::session::Session;
use shamir_query_types::admin::activity::{CancelOutcome, RequestExecId};
use shamir_query_types::wire::DbResponse;
use shamir_tx::CancelState;
use shamir_types::access::{Action, ResourcePath};

use super::requests::RequestEntry;
use crate::engine::db_handler::handler::{session_actor, ShamirDbHandler};
use crate::engine::db_handler::repo_audit::enqueue_cancel_refusal_audit;

pub(super) fn cancel_entry(entry: &RequestEntry) -> CancelOutcome {
    if !entry.cancellable {
        return match entry.cancel.state() {
            CancelState::Done | CancelState::Cancelled => CancelOutcome::AlreadyFinished,
            CancelState::Running | CancelState::Committing => CancelOutcome::NotCancellable,
        };
    }
    match entry.cancel.try_cancel() {
        Ok(()) => CancelOutcome::Cancelled,
        Err(CancelState::Committing) => CancelOutcome::AlreadyCommitting,
        Err(CancelState::Done | CancelState::Cancelled) => CancelOutcome::AlreadyFinished,
        Err(CancelState::Running) => unreachable!("running state cannot refuse cancellation"),
    }
}

impl ShamirDbHandler {
    /// Called on an authenticated control connection. Looking up the row
    /// reveals ownership internally, but no outcome is read or returned
    /// for another user's row before the ActivityView permission check.
    pub(in crate::engine::db_handler) async fn cancel_request(
        &self,
        session: &Session,
        target: RequestExecId,
    ) -> DbResponse {
        let Some(entry) = self.activity_requests.get(target.0) else {
            return DbResponse::CancelOutcome {
                outcome: CancelOutcome::AlreadyFinished,
            };
        };
        if entry.user_id != session.user_id
            && self
                .db
                .authorize_access(
                    &session_actor(session),
                    &ResourcePath::ActivityView,
                    Action::Manage,
                )
                .await
                .is_err()
        {
            return DbResponse::Error {
                code: "permission_denied".to_owned(),
                message:
                    "cancelling another user's request requires ActivityView manage permission"
                        .to_owned(),
            };
        }

        let outcome = cancel_entry(&entry);
        // Lookup has released its epoch guard. The state decision above does
        // not depend on audit I/O.
        //
        // The reply is built AFTER this synchronous enqueue, so its real cost
        // is the enqueue's cost: one per-principal counter update, one
        // `spawn_blocking` and one short push+prune of the queue's handle list.
        // None of that waits for the audit writer, its persistence gate or a
        // flush — a refusal that cannot be admitted is dropped and counted
        // instead of blocking the reply.
        if let Some(audit) = &self.audit {
            // The job is detached: the append runs on a blocking thread and
            // this reply never awaits it.
            drop(enqueue_cancel_refusal_audit(
                &self.cancel_refusal_audit,
                audit,
                session,
                target,
                &outcome,
            ));
        }
        DbResponse::CancelOutcome { outcome }
    }
}
