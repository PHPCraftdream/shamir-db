use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::Verified;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_transport_http::{ExecOutcome, ExecStatus, RestExecutor};

use crate::db_handler::ShamirDbHandler;
use crate::security::tokens::TokenService;

use super::bucket::{BucketCache, BucketError};
use super::scope::{classify_request, Decision};
use super::RestGateRails;

type OwnerName = dyn Fn(&[u8; 16]) -> String + Send + Sync;
pub(super) type NativeHandler = dyn for<'a> Fn(
        &'a Session,
        DbRequest,
        &'a ConnectionServices,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>
    + Send
    + Sync;

/// Native typed handler adapter for requests accepted by REST scope policy.
pub struct ScopedHandler {
    handler: Arc<NativeHandler>,
}

impl ScopedHandler {
    /// Wrap the concrete native database handler.
    pub fn new(handler: Arc<ShamirDbHandler>) -> Self {
        Self {
            handler: Arc::new(move |session, request, conn| {
                let handler = Arc::clone(&handler);
                Box::pin(async move { handler.handle_decoded(session, request, conn).await })
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn with_handler(handler: Arc<NativeHandler>) -> Self {
        Self { handler }
    }

    pub(super) async fn handle(
        &self,
        session: &Session,
        request: DbRequest,
        conn: &ConnectionServices,
    ) -> Result<Vec<u8>, HandleDenial> {
        let shamir_connect::server::SessionOrigin::Token(origin) = &session.origin else {
            return Err(HandleDenial::Forbidden("token_request_denied"));
        };
        let (mode, _, databases) = origin.scope();
        let decision = classify_request(&request, mode, databases);
        match decision {
            Decision::Allowed(_) => {}
            Decision::Denied(code) => return Err(HandleDenial::Forbidden(code)),
            Decision::NotSupported(code) => return Err(HandleDenial::BadRequest(code)),
        }
        (self.handler)(session, request, conn)
            .await
            .map_err(|_| HandleDenial::ApplicationError)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HandleDenial {
    Forbidden(&'static str),
    BadRequest(&'static str),
    ApplicationError,
}

/// Concrete REST executor over the native database handler.
pub struct RestHandler {
    service: Arc<TokenService>,
    handler: ScopedHandler,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    username: Arc<OwnerName>,
    buckets: Arc<BucketCache>,
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
        let buckets = Arc::new(BucketCache::default());
        let gate = super::RestGate::new_with_buckets(
            Arc::clone(&service),
            Arc::clone(&clock),
            rails,
            Arc::clone(&buckets),
        );
        metrics::describe_counter!(
            "shamir_token_request_denied_total",
            metrics::Unit::Count,
            "REST token requests denied by endpoint scope policy"
        );
        metrics::counter!("shamir_token_request_denied_total", "endpoint" => "rest").increment(0);
        metrics::describe_histogram!(
            "shamir_token_request_duration_seconds",
            metrics::Unit::Seconds,
            "REST token request execution duration"
        );
        let executor = Self {
            service,
            handler: ScopedHandler::new(handler),
            clock,
            username,
            buckets,
        };
        (gate, executor)
    }

    fn rate_limited(retry: u32) -> ExecOutcome {
        let seconds = retry.max(1);
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
        let prepared = classify_request(&request, record.mode, record.databases.as_deref());
        let operation = match prepared {
            Decision::Denied(code) => {
                metrics::counter!("shamir_token_request_denied_total", "endpoint" => "rest")
                    .increment(1);
                return Self::error(code, ExecStatus::Forbidden);
            }
            Decision::NotSupported(code) => return Self::error(code, ExecStatus::BadRequest),
            Decision::Allowed(operation) => operation,
        };
        tracing::trace!(operation = operation.label(), "rest request admitted");
        let (_, session) =
            match self
                .buckets
                .session_for(grant, (self.username)(&record.owner_user_id), now_ns)
            {
                Ok(pair) => pair,
                Err(BucketError::RateLimited(retry)) => return Self::rate_limited(retry),
                Err(BucketError::Unavailable) => {
                    return Self::error("server_busy", ExecStatus::Unavailable)
                }
            };
        // REST creates no subscription and disables push; conn_id is unused by this request path.
        let conn = ConnectionServices::without_push(0);
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
            Err(HandleDenial::Forbidden(code)) => {
                metrics::counter!("shamir_token_request_denied_total", "endpoint" => "rest")
                    .increment(1);
                Self::error(code, ExecStatus::Forbidden)
            }
            Err(HandleDenial::BadRequest(code)) => Self::error(code, ExecStatus::BadRequest),
            Err(HandleDenial::ApplicationError) => Self::error("application_error", ExecStatus::Ok),
        }
    }
}
