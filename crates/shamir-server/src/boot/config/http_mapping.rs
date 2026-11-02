use std::time::Duration;

use shamir_transport_http::{BodyBudget, EndpointSet, HttpLimits, HttpServerConfig};

use super::HttpListenerSettings;

/// Creates `HttpServerConfig` limits from HTTP listener settings.
pub fn to_http_server_config(
    settings: &HttpListenerSettings,
    endpoints: EndpointSet,
    body_budget: std::sync::Arc<BodyBudget>,
    auth_init_timeout_ms: u64,
) -> HttpServerConfig {
    let mut limits = HttpLimits::new(settings.max_request_body_bytes, body_budget);
    limits.head_timeout =
        Duration::from_millis(settings.head_timeout_ms.unwrap_or(auth_init_timeout_ms));
    limits.body_timeout = Duration::from_millis(settings.body_timeout_ms);
    limits.request_timeout = Duration::from_millis(settings.request_timeout_ms);
    limits.write_timeout = limits.request_timeout;
    HttpServerConfig {
        limits,
        endpoints,
        tls: None,
        hsts: settings.tls,
        forbidden_body: std::sync::Arc::new(|_| bytes::Bytes::new()),
        ping_body: bytes::Bytes::new(),
    }
}
