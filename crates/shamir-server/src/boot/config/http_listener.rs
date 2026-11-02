use std::net::SocketAddr;

use serde::Deserialize;

use crate::boot::config::{ConfigError, ListenerConfig, ListenerKind};
use crate::security::perimeter::http_ws::browser_policy::canonical_authority;

/// HTTP listener policy and transport limits.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HttpListenerSettings {
    /// Hard request body cap in bytes.
    #[serde(default = "default_http_body_bytes")]
    pub max_request_body_bytes: usize,
    /// Head timeout override in milliseconds; absent inherits auth_init_timeout_ms.
    #[serde(default)]
    pub head_timeout_ms: Option<u64>,
    /// Maximum body receive duration in milliseconds.
    #[serde(default = "default_http_body_timeout_ms")]
    pub body_timeout_ms: u64,
    /// End-to-end request deadline in milliseconds.
    #[serde(default = "default_http_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// Enables TLS; transport config enforces TLS 1.3, ALPN, and no early data.
    #[serde(default)]
    pub tls: Option<bool>,
    /// Allows plaintext binding outside loopback.
    #[serde(default)]
    pub allow_plaintext_remote: bool,
    /// Allows read-write tokens on remote plaintext.
    #[serde(default)]
    pub allow_plaintext_write: bool,
    /// Opts into remote self-signed certificate use. Runtime refusal and SPKI pin printing are wired in T3b-3.
    #[serde(default)]
    pub allow_self_signed: bool,
    /// CIDR list of proxies trusted for REST forwarded-address rate limits.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// Trusted browser Host authorities; entries must use canonical form.
    #[serde(default)]
    pub host_authority_allowlist: Vec<String>,
}

impl Default for HttpListenerSettings {
    fn default() -> Self {
        Self {
            max_request_body_bytes: default_http_body_bytes(),
            head_timeout_ms: None,
            body_timeout_ms: default_http_body_timeout_ms(),
            request_timeout_ms: default_http_request_timeout_ms(),
            tls: None,
            allow_plaintext_remote: false,
            allow_plaintext_write: false,
            allow_self_signed: false,
            trusted_proxies: Vec::new(),
            host_authority_allowlist: Vec::new(),
        }
    }
}

impl HttpListenerSettings {
    /// Validates HTTP policy, required TLS selection, and listener limits.
    pub fn validate(
        &self,
        idx: usize,
        listener: &ListenerConfig,
        auth_init_timeout_ms: u64,
    ) -> Result<(), ConfigError> {
        let invalid =
            |message: String| ConfigError::Validation(format!("listeners[{idx}]: {message}"));
        if listener.kind != ListenerKind::Http {
            return Err(invalid(
                "HTTP policy validation called for non-HTTP listener".into(),
            ));
        }
        let addr: SocketAddr = listener
            .addr
            .parse()
            .map_err(|error| invalid(format!("invalid HTTP bind address: {error}")))?;
        if self.max_request_body_bytes == 0 {
            return Err(invalid(
                "max_request_body_bytes must be greater than zero".into(),
            ));
        }
        let head_timeout_ms = self.head_timeout_ms.unwrap_or(auth_init_timeout_ms);
        if head_timeout_ms == 0 || self.body_timeout_ms == 0 || self.request_timeout_ms == 0 {
            return Err(invalid(format!(
                "HTTP head/body/request timeout must be greater than zero (effective head_timeout_ms={head_timeout_ms} ms from {}, body_timeout_ms={} ms, request_timeout_ms={} ms)",
                if self.head_timeout_ms.is_some() { "listener override" } else { "security.connection.auth_init_timeout_ms inheritance" },
                self.body_timeout_ms,
                self.request_timeout_ms,
            )));
        }
        if self.body_timeout_ms > self.request_timeout_ms
            || head_timeout_ms > self.request_timeout_ms
        {
            return Err(invalid(format!(
                "HTTP timeout limits invalid: effective head_timeout_ms={head_timeout_ms} ms (from {}), body_timeout_ms={} ms, request_timeout_ms={} ms; head and body must not exceed request",
                if self.head_timeout_ms.is_some() { "listeners[].head_timeout_ms" } else { "security.connection.auth_init_timeout_ms inheritance" },
                self.body_timeout_ms,
                self.request_timeout_ms,
            )));
        }
        if listener.profile.is_some() {
            return Err(invalid("profile must be absent for kind=http".into()));
        }
        if self.tls.is_none() {
            return Err(invalid("kind=http requires tls".into()));
        }
        let tls = self.tls == Some(true);
        if self.allow_plaintext_write && !self.allow_plaintext_remote {
            return Err(invalid(
                "allow_plaintext_write requires allow_plaintext_remote".into(),
            ));
        }
        if !tls && !addr.ip().is_loopback() && !self.allow_plaintext_remote {
            return Err(invalid(format!(
                "plaintext HTTP bind {} is not loopback; set allow_plaintext_remote explicitly",
                addr
            )));
        }
        if tls && (self.allow_plaintext_remote || self.allow_plaintext_write) {
            return Err(invalid(
                "plaintext opt-in flags are only valid when tls is false".into(),
            ));
        }
        for authority in &self.host_authority_allowlist {
            let canonical = canonical_authority(authority, tls);
            if canonical.as_deref() != Some(authority.as_str()) {
                return Err(invalid(format!(
                    "host_authority_allowlist entry `{authority}` is not canonical for this listener scheme"
                )));
            }
        }
        if !listener.browser_origin_allowlist.is_empty() {
            shamir_transport_ws::BrowserOriginPolicy::try_allow(
                listener.browser_origin_allowlist.iter().cloned(),
            )
            .map_err(|error| invalid(format!("invalid browser_origin_allowlist: {error}")))?;
        }
        for proxy in &self.trusted_proxies {
            let (address, prefix) = proxy
                .split_once('/')
                .ok_or_else(|| invalid(format!("trusted_proxies entry `{proxy}` must be CIDR")))?;
            let ip: std::net::IpAddr = address.parse().map_err(|error| {
                invalid(format!(
                    "trusted_proxies entry `{proxy}` has invalid address: {error}"
                ))
            })?;
            let bits = if ip.is_ipv4() { 32 } else { 128 };
            let prefix: u8 = prefix.parse().map_err(|error| {
                invalid(format!(
                    "trusted_proxies entry `{proxy}` has invalid prefix: {error}"
                ))
            })?;
            if prefix > bits {
                return Err(invalid(format!(
                    "trusted_proxies entry `{proxy}` prefix exceeds {bits}"
                )));
            }
        }
        Ok(())
    }
}

fn default_http_body_bytes() -> usize {
    16 * 1024 * 1024
}
fn default_http_body_timeout_ms() -> u64 {
    30_000
}
fn default_http_request_timeout_ms() -> u64 {
    120_000
}
