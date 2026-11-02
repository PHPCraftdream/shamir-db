use crate::budget::BodyBudget;
use bytes::Bytes;
use rustls::ServerConfig;
use std::sync::Arc;
use std::time::Duration;

/// HTTP endpoint type presented to the authentication gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endpoint {
    /// REST request endpoint.
    Rest,
    /// Reserved WebSocket endpoint.
    Ws,
}

/// Routes enabled by this listener.
#[derive(Clone, Copy, Debug)]
pub struct EndpointSet {
    /// Whether REST routes are enabled.
    pub rest: bool,
    /// Whether the WebSocket route is enabled.
    pub ws: bool,
}

/// Transport resource and deadline limits.
#[derive(Clone, Debug)]
pub struct HttpLimits {
    /// Maximum request-head size in bytes.
    pub max_head_bytes: usize,
    /// Maximum number of request headers.
    pub max_headers: usize,
    /// Maximum request-body size in bytes.
    pub max_body_bytes: usize,
    /// Deadline for receiving the request head.
    pub head_timeout: Duration,
    /// Deadline for receiving the complete request body.
    pub body_timeout: Duration,
    /// Deadline for processing the complete request.
    pub request_timeout: Duration,
    /// Shared budget for admitted request-body bytes.
    pub body_budget: Arc<BodyBudget>,
    /// Per-response write deadline, normally equal to request_timeout.
    pub write_timeout: Duration,
}

impl HttpLimits {
    /// Limits using protocol time defaults and a caller-supplied body cap/budget.
    pub fn new(max_body_bytes: usize, body_budget: Arc<BodyBudget>) -> Self {
        Self {
            max_head_bytes: 8192,
            max_headers: 64,
            max_body_bytes,
            head_timeout: Duration::from_secs(5),
            body_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(120),
            body_budget,
            write_timeout: Duration::from_secs(120),
        }
    }
}

/// Complete generic HTTP listener configuration.
#[derive(Clone)]
pub struct HttpServerConfig {
    /// Resource and deadline limits.
    pub limits: HttpLimits,
    /// Enabled listener endpoints.
    pub endpoints: EndpointSet,
    /// Optional server TLS configuration.
    pub tls: Option<Arc<ServerConfig>>,
    /// Whether to add HSTS on TLS responses.
    pub hsts: bool,
    /// Fixed caller-defined body for gate denials carrying an application code.
    pub forbidden_body: Arc<dyn Fn(&str) -> Bytes + Send + Sync>,
    /// MessagePack Ping request passed to the REST executor.
    pub ping_body: Bytes,
}
