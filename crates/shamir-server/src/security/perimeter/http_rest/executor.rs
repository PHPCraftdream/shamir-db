use std::sync::Arc;

use bytes::Bytes;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::Verified;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_transport_http::{ExecOutcome, ExecStatus, RestExecutor};

use crate::db_handler::ShamirDbHandler;
use crate::security::tokens::TokenService;

use super::bucket::BucketCache;
use super::scope::{classify_request, Decision};
use super::RestGateRails;

type OwnerName = dyn Fn(&[u8; 16]) -> String + Send + Sync;

/// Native typed handler adapter for requests accepted by REST scope policy.
pub struct ScopedHandler {
    handler: Arc<ShamirDbHandler>,
}

impl ScopedHandler {
    /// Wrap the concrete native database handler.
    pub fn new(handler: Arc<ShamirDbHandler>) -> Self {
        Self { handler }
    }

    pub(crate) fn classify(
        &self,
        request: &DbRequest,
        mode: shamir_connect::server::token::TokenMode,
        databases: Option<&[String]>,
    ) -> Decision {
        classify_request(request, mode, databases)
    }

    pub(crate) async fn handle(
        &self,
        session: &Session,
        request: DbRequest,
        conn: &ConnectionServices,
    ) -> Result<Vec<u8>, String> {
        self.handler.handle_decoded(session, request, conn).await
    }
}

/// Concrete REST executor over the native database handler.
pub struct RestHandler {
    service: Arc<TokenService>,
    handler: ScopedHandler,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    username: Arc<OwnerName>,
    plaintext_remote: bool,
    allow_plaintext_write: bool,
    buckets: BucketCache,
}

impl RestHandler {
    /// Constructs the REST gate and executor for T3b-3 listener wiring.
    pub fn new(
        service: Arc<TokenService>,
        handler: Arc<ShamirDbHandler>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        username: Arc<OwnerName>,
        rails: RestGateRails,
    ) -> (super::RestGate, Self) {
        let gate = super::RestGate::new(Arc::clone(&service), Arc::clone(&clock), rails);
        let executor = Self {
            service,
            handler: ScopedHandler::new(handler),
            clock,
            username,
            plaintext_remote: rails.plaintext_remote,
            allow_plaintext_write: rails.allow_plaintext_write,
            buckets: BucketCache::default(),
        };
        (gate, executor)
    }

    fn rate_limited(retry: u32) -> ExecOutcome {
        let seconds = retry.div_ceil(1000).max(1);
        let mut outcome = Self::error("rate_limited", ExecStatus::RateLimited);
        outcome.retry_after_secs = Some(u64::from(seconds));
        outcome
    }

    fn error(code: &'static str, status: ExecStatus) -> ExecOutcome {
        let response = DbResponse::Error {
            code: code.to_string(),
            message: code.to_string(),
        };
        let body = rmp_serde::to_vec_named(&response)
            .map(Bytes::from)
            .unwrap_or_else(|_| Bytes::from_static(b"\x81\xa4kind\xa5error"));
        ExecOutcome {
            status,
            body,
            retry_after_secs: None,
            hold: None,
        }
    }
}

impl RestExecutor<Verified> for RestHandler {
    async fn execute(&self, grant: &Verified, body: Bytes) -> ExecOutcome {
        let started = std::time::Instant::now();
        let Some(now_ns) = (self.clock)() else {
            return Self::error("server_busy", ExecStatus::Unavailable);
        };
        if !self.service.still_valid(grant, now_ns) {
            return Self::error("unauthorized", ExecStatus::Unauthorized);
        }
        if shamir_query_types::validate_request_alias_caps(&body).is_err() {
            return Self::error("invalid_request", ExecStatus::BadRequest);
        }
        let request: DbRequest = match rmp_serde::from_slice(&body) {
            Ok(request) => request,
            Err(_) => return Self::error("invalid_request", ExecStatus::BadRequest),
        };
        let record = grant.state.record();
        if self.plaintext_remote
            && !self.allow_plaintext_write
            && record.mode == shamir_connect::server::token::TokenMode::ReadWrite
        {
            return Self::error("token_plaintext_write_denied", ExecStatus::Forbidden);
        }
        match self
            .handler
            .classify(&request, record.mode, record.databases.as_deref())
        {
            Decision::Denied(code) => return Self::error(code, ExecStatus::Forbidden),
            Decision::NotSupported(code) => return Self::error(code, ExecStatus::BadRequest),
            Decision::Allowed(_) => {}
        }
        let (session_id, session) =
            match self
                .buckets
                .session_for(grant, (self.username)(&record.owner_user_id), now_ns)
            {
                Ok(pair) => pair,
                Err(retry) => return Self::rate_limited(retry),
            };
        let conn = ConnectionServices::without_push(u64::from(session_id[0]));
        let Some(execution_now_ns) = (self.clock)() else {
            return Self::error("server_busy", ExecStatus::Unavailable);
        };
        if !self.service.still_valid(grant, execution_now_ns) {
            return Self::error("unauthorized", ExecStatus::Unauthorized);
        }
        let handler = self.handler.handle(&session, request, &conn);
        let (result, hold) = crate::byte_budget::run_with_handoff(handler).await;
        metrics::histogram!("shamir_token_request_duration_seconds", "endpoint" => "rest")
            .record(started.elapsed().as_secs_f64());
        match result {
            Ok(response) => ExecOutcome {
                status: ExecStatus::Ok,
                body: Bytes::from(response),
                retry_after_secs: None,
                hold: hold.map(|guard| Box::new(guard) as Box<dyn Send + Sync>),
            },
            Err(_) => Self::error("application_error", ExecStatus::Ok),
        }
    }
}
