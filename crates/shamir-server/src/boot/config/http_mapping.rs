use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use rustls::ServerConfig;
use shamir_transport_http::{BodyBudget, EndpointSet, HttpLimits, HttpServerConfig};

use super::HttpListenerSettings;
use crate::boot::config::ConfigError;
/// Creates `HttpServerConfig` from listener settings and caller-provided protocol bodies.
pub fn to_http_server_config(
    settings: &HttpListenerSettings,
    endpoints: EndpointSet,
    body_budget: Arc<BodyBudget>,
    auth_init_timeout_ms: u64,
    tls: Option<Arc<ServerConfig>>,
    forbidden_body: Arc<dyn Fn(&str) -> Bytes + Send + Sync>,
    ping_body: Bytes,
) -> Result<HttpServerConfig, ConfigError> {
    if settings.tls != Some(tls.is_some()) {
        return Err(ConfigError::Validation(
            "HTTP listener tls setting does not agree with TLS configuration".into(),
        ));
    }
    let mut limits = HttpLimits::new(settings.max_request_body_bytes, body_budget);
    limits.head_timeout =
        Duration::from_millis(settings.head_timeout_ms.unwrap_or(auth_init_timeout_ms));
    limits.body_timeout = Duration::from_millis(settings.body_timeout_ms);
    limits.request_timeout = Duration::from_millis(settings.request_timeout_ms);
    limits.connection_drain_timeout = Some(Duration::from_secs(5));
    limits.write_timeout = limits.request_timeout;
    Ok(HttpServerConfig {
        limits,
        endpoints,
        tls,
        hsts: settings.tls == Some(true),
        forbidden_body,
        ping_body,
    })
}
