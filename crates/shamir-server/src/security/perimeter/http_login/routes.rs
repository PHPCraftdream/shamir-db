//! Application implementation of the transport's HTTP-login hook.

use super::admission::{AdmissionError, InputPermit, LoginHeadPolicy};
use super::service::{begin, finish, LoginServiceContext};
use super::wire::{response, LoginResponse};
use bytes::Bytes;
use shamir_transport_http::gate::PeerInfo;
use shamir_transport_http::{LoginHead, LoginResponse as TransportResponse, LoginRoutes};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::Instrument;

/// Listener-specific HTTP login route service.
pub struct HttpLoginRoutes {
    enabled: bool,
    context: Arc<LoginServiceContext>,
    policy: LoginHeadPolicy,
    tasks: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
    stopping: AtomicBool,
}

impl HttpLoginRoutes {
    /// Create a listener service from shared dependencies and validated policy.
    pub fn new(enabled: bool, context: Arc<LoginServiceContext>, policy: LoginHeadPolicy) -> Self {
        Self {
            enabled,
            context,
            policy,
            tasks: tokio::sync::Mutex::new(tokio::task::JoinSet::new()),
            stopping: AtomicBool::new(false),
        }
    }

    /// Drain in-flight login work without cancelling critical storage operations.
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let mut tasks = self.tasks.lock().await;
        while tasks.join_next().await.is_some() {}
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
            return Err(Self::error(503, "server_busy", Some(1)));
        }
        if let Err((status, code)) = self.validate_head(&head) {
            return Err(Self::error(status, code, None));
        }
        let source = match self.policy.resolve(&head, &peer) {
            Ok(source) => source,
            Err(error) => return Err(Self::error(error.status, error.code, None)),
        };
        let limit = if route.ends_with(b"begin") { 4096 } else { 256 };
        if let Some(length) = head.header_values(b"content-length").next() {
            if std::str::from_utf8(length)
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .is_none_or(|n| n > limit)
            {
                return Err(Self::error(413, "request_too_large", None));
            }
        }
        self.context
            .admission
            .admit(source)
            .await
            .map_err(|error| match error {
                AdmissionError::RateLimited {
                    retry_after_seconds,
                } => Self::error(429, "rate_limited", Some(retry_after_seconds)),
                AdmissionError::SourceSlots => Self::error(429, "rate_limited", Some(1)),
                AdmissionError::ServerBusy => Self::error(503, "server_busy", Some(1)),
            })
    }

    async fn handle(&self, route: Bytes, permit: Self::Permit, body: Bytes) -> TransportResponse {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let context = Arc::clone(&self.context);
        let future = async move {
            let source = permit.source();
            let value = if route.ends_with(b"begin") {
                begin(&context, source, &body).await
            } else {
                finish(&context, source, &body).await
            };
            let reply = Self::reply(value);
            drop(permit);
            // A missing receiver means the peer disconnected; response delivery is uncertain.
            let _ = sender.send(reply);
        };
        {
            let mut tasks = self.tasks.lock().await;
            while tasks.try_join_next().is_some() {}
            tasks.spawn(future.instrument(tracing::Span::current()));
        }
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
            .finish()
    }
}
