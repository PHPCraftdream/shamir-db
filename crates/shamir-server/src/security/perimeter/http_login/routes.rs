//! Application implementation of the transport's HTTP-login hook.

use super::admission::{AdmissionError, InputPermit, LoginHeadPolicy};
use super::metrics::{record_result, LoginResult};
use super::service::{begin, finish, LoginServiceContext};
use super::wire::{response, LoginResponse};
use bytes::Bytes;
use shamir_transport_http::gate::PeerInfo;
use shamir_transport_http::{LoginHead, LoginResponse as TransportResponse, LoginRoutes};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tracing::Instrument;

/// Upper bound of the login worker drain at shutdown (see `worker_drain_budget`).
const WORKER_DRAIN_CAP: Duration = Duration::from_secs(7);

/// Tracks detached route workers independently of the listener's lifetime.
struct InFlight {
    count: AtomicUsize,
    idle: tokio::sync::Notify,
}

struct InFlightGuard(Arc<InFlight>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

fn route_result(route: &[u8]) -> &'static str {
    if route.ends_with(b"begin") {
        "begin"
    } else {
        "finish"
    }
}

fn result_for_status(status: u16) -> LoginResult {
    match status {
        403 => LoginResult::PolicyDenied,
        429 => LoginResult::RateLimited,
        503 => LoginResult::ServerBusy,
        400 | 404 | 405 | 413 | 415 => LoginResult::InvalidRequest,
        _ => LoginResult::AuthenticationFailed,
    }
}

/// Listener-specific HTTP login route service.
pub struct HttpLoginRoutes {
    enabled: bool,
    context: Arc<LoginServiceContext>,
    policy: LoginHeadPolicy,
    in_flight: Arc<InFlight>,
    stopping: AtomicBool,
}

impl HttpLoginRoutes {
    /// Describe the browser Origin policy without exposing configured origins.
    pub(crate) fn describe_origin_policy(count: usize) -> String {
        if count == 0 {
            "HTTP login Origin policy: requests without Origin are allowed; requests with Origin are denied until an allowlist is configured".to_string()
        } else {
            format!(
                "HTTP login Origin policy configured (configured_origins={count}; origins omitted)"
            )
        }
    }

    /// Bound worker draining by the HTTP transport request deadline, capped at 7 seconds:
    /// the server's outer listener deadline is 10 seconds and the audit consumer needs
    /// 2 seconds after the workers, so a longer drain would only be preempted by an abort.
    pub(crate) fn worker_drain_budget(request_timeout_ms: u64) -> Duration {
        Duration::from_millis(request_timeout_ms).min(WORKER_DRAIN_CAP)
    }

    /// Create a listener service from shared dependencies and validated policy.
    pub fn new(enabled: bool, context: Arc<LoginServiceContext>, policy: LoginHeadPolicy) -> Self {
        Self {
            enabled,
            context,
            policy,
            in_flight: Arc::new(InFlight {
                count: AtomicUsize::new(0),
                idle: tokio::sync::Notify::new(),
            }),
            stopping: AtomicBool::new(false),
        }
    }

    /// Waits at most `budget` for detached login work to finish.
    /// Returns `false` when work continues detached; cancelling this wait does not cancel workers.
    /// cancel-safe: yes — cancellation stops waiting, not detached workers.
    pub async fn shutdown(&self, budget: Duration) -> bool {
        self.stopping.store(true, Ordering::SeqCst);
        let idle = async {
            loop {
                let notified = self.in_flight.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.in_flight.count.load(Ordering::SeqCst) == 0 {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(budget, idle).await.is_ok()
    }

    fn error(status: u16, code: &str, retry: Option<u32>) -> TransportResponse {
        match response(status, code, retry) {
            Ok(value) => value,
            Err(_) => TransportResponse::with_retry_after(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Bytes::from_static(b"\x82\xa5error\xabserver_busy\xabretry_after\x01"),
                1,
            ),
        }
    }

    fn reply(value: LoginResponse) -> TransportResponse {
        value
    }

    fn validate_head(&self, head: &LoginHead<'_>) -> Result<(), (u16, &'static str)> {
        if !self.enabled {
            return Err((404, "not_found"));
        }
        if head.method() != b"POST" {
            return Err((405, "method_not_allowed"));
        }
        let content_types: Vec<_> = head.header_values(b"content-type").collect();
        if content_types.len() != 1
            || content_types[0] != b"application/msgpack"
            || head.header_values(b"content-encoding").next().is_some()
        {
            return Err((415, "unsupported_media_type"));
        }
        let lengths: Vec<_> = head.header_values(b"content-length").collect();
        let encodings: Vec<_> = head.header_values(b"transfer-encoding").collect();
        if lengths.len() > 1
            || encodings.len() > 1
            || (!lengths.is_empty() && !encodings.is_empty())
        {
            return Err((400, "invalid_request"));
        }
        Ok(())
    }
}

impl LoginRoutes for HttpLoginRoutes {
    type Permit = InputPermit;
    fn enabled(&self) -> bool {
        self.enabled
    }

    async fn admit_head(
        &self,
        route: &[u8],
        head: LoginHead<'_>,
        peer: PeerInfo,
    ) -> Result<Self::Permit, TransportResponse> {
        if self.stopping.load(Ordering::Acquire) {
            record_result(route_result(route), LoginResult::ServerBusy);
            return Err(Self::error(503, "server_busy", Some(1)));
        }
        if let Err((status, code)) = self.validate_head(&head) {
            record_result(route_result(route), result_for_status(status));
            return Err(Self::error(status, code, None));
        }
        let source = match self.policy.resolve(&head, &peer) {
            Ok(source) => source,
            Err(error) => {
                record_result(
                    route_result(route),
                    match error.code {
                        "authority_forbidden" | "origin_forbidden" | "proxy_forbidden" => {
                            LoginResult::PolicyDenied
                        }
                        _ => LoginResult::InvalidRequest,
                    },
                );
                return Err(Self::error(error.status, error.code, None));
            }
        };
        let limit = if route.ends_with(b"begin") { 4096 } else { 256 };
        if let Some(length) = head.header_values(b"content-length").next() {
            if std::str::from_utf8(length)
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .is_none_or(|n| n > limit)
            {
                record_result(route_result(route), LoginResult::InvalidRequest);
                return Err(Self::error(413, "request_too_large", None));
            }
        }
        self.context.admission.admit(source).await.map_err(|error| {
            let result = match error {
                AdmissionError::RateLimited { .. } | AdmissionError::SourceSlots => {
                    LoginResult::RateLimited
                }
                AdmissionError::ServerBusy => LoginResult::ServerBusy,
            };
            record_result(route_result(route), result);
            match error {
                AdmissionError::RateLimited {
                    retry_after_seconds,
                } => Self::error(429, "rate_limited", Some(retry_after_seconds)),
                AdmissionError::SourceSlots => Self::error(429, "rate_limited", Some(1)),
                AdmissionError::ServerBusy => Self::error(503, "server_busy", Some(1)),
            }
        })
    }

    async fn handle(&self, route: Bytes, permit: Self::Permit, body: Bytes) -> TransportResponse {
        // Register before checking stopping. SeqCst order ensures a racing
        // shutdown either observes this worker or this request observes stop.
        self.in_flight.count.fetch_add(1, Ordering::SeqCst);
        let guard = InFlightGuard(Arc::clone(&self.in_flight));
        if self.stopping.load(Ordering::SeqCst) {
            drop(guard);
            drop(permit);
            return Self::error(503, "server_busy", Some(1));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let context = Arc::clone(&self.context);
        let future = async move {
            let _guard = guard;
            let source = permit.source();
            let value = if route.ends_with(b"begin") {
                begin(&context, source, &body).await
            } else {
                finish(&context, source, &body).await
            };
            let reply = Self::reply(value);
            drop(permit);
            // A missing receiver means the peer disconnected; the response outcome is uncertain.
            let _ = sender.send(reply);
        };
        // Detached intentionally: listener abort must not cancel token
        // issuance between create and its verification/compensation. A false
        // shutdown result leaves outstanding workers running detached, and
        // cancelling the shutdown wait does not cancel them.
        tokio::spawn(future.instrument(tracing::Span::current()));
        match receiver.await {
            Ok(reply) => reply,
            Err(_) => Self::error(503, "server_busy", Some(1)),
        }
    }

    fn body_limit(&self, route: &[u8]) -> usize {
        if route.ends_with(b"begin") {
            4096
        } else {
            256
        }
    }
}

impl core::fmt::Debug for HttpLoginRoutes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HttpLoginRoutes")
            .field("enabled", &self.enabled)
            .field("context", &"<redacted>")
            .field("policy", &"<redacted>")
            .field("stopping", &self.stopping.load(Ordering::Relaxed))
            .finish()
    }
}
