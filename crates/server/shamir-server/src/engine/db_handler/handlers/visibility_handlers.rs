use shamir_connect::server::session::Session;
use shamir_db::query::batch::BatchError;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{ResyncRequiredDetail, VisibilityWait};
use shamir_query_types::wire::DbResponse;

use crate::db_handler::cursor_handlers::error_response;
use crate::db_handler::handler::{session_actor, ShamirDbHandler};

/// S5c-X PR8b: cursor reads (`CreateCursor`/`FetchNext`) bypass shamir-db's
/// batch read gate, so the `resync_required` quarantine is probed here
/// directly. One atomic-map probe — the empty-cache fast path lives inside
/// the cache itself.
///
/// `alias` has no batch-op name on the cursor path; `"cursor"` labels the
/// quarantined read in the wire message (see `BatchError::ResyncRequired`'s
/// `Display`: `query '{alias}': repository db/repo is quarantined ...`).
pub(crate) fn quarantine_cursor_error(
    db: &ShamirDb,
    db_name: &str,
    repo_name: &str,
) -> Option<BatchError> {
    db.read_quarantine_reason(db_name, repo_name)
        .map(|reason| BatchError::ResyncRequired {
            alias: "cursor".to_string(),
            detail: Box::new(ResyncRequiredDetail {
                db: db_name.to_string(),
                repo: repo_name.to_string(),
                reason,
            }),
        })
}

impl ShamirDbHandler {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn wait_for_visibility(
        &self,
        session: &Session,
        query_version: u32,
        db: &str,
        repo: &str,
        version: u64,
        wait: VisibilityWait,
    ) -> DbResponse {
        if let Err(e) = crate::version::check_query_lang(query_version) {
            return DbResponse::Error {
                code: "unsupported_query_version".into(),
                message: e.to_string(),
            };
        }
        let wait = match self.transactions.resolve_wait(wait) {
            Ok(wait) => wait,
            Err(e) => {
                return DbResponse::Error {
                    code: e.code().unwrap_or("validation").into(),
                    message: e.to_string(),
                };
            }
        };
        match self
            .db
            .wait_for_visibility(session_actor(session), db, repo, version, wait)
            .await
        {
            Ok(visibility) => DbResponse::VisibilityObserved {
                version,
                visibility,
            },
            Err(e) => error_response(&e),
        }
    }

    pub fn with_transactions(mut self, transactions: crate::config::TransactionsConfig) -> Self {
        self.transactions = transactions;
        self
    }

    pub(crate) fn execution_policy(&self) -> shamir_query_types::batch::ExecPolicy {
        let mut policy = self.query_limits.policy();
        policy.read_visibility_timeout_ms = self.transactions.read_visibility_timeout_ms;
        policy
    }
}
