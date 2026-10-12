use shamir_connect::server::session::Session;
use shamir_query_types::batch::VisibilityWait;
use shamir_query_types::wire::DbResponse;

use super::cursor_handlers::error_response;
use super::handler::{session_actor, ShamirDbHandler};

impl ShamirDbHandler {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn wait_for_visibility(
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

    pub(super) fn execution_policy(&self) -> shamir_query_types::batch::ExecPolicy {
        let mut policy = self.query_limits.policy();
        policy.read_visibility_timeout_ms = self.transactions.read_visibility_timeout_ms;
        policy
    }
}
