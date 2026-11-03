use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;

use crate::boot::config::{ConfigError, ListenerConfig, ListenerKind};

use super::login_listener::{canonical_authority, parse_listener_id};

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
    /// The public edge terminates TLS before forwarding to this loopback listener.
    #[serde(default)]
    pub external_tls: bool,
    /// Password login configuration.
    #[serde(default)]
    pub login: super::login_listener::LoginListenerSettings,
    /// Opts into remote self-signed certificate use. Runtime refusal and SPKI pin printing are wired in T3b-3.
    #[serde(default)]
    pub allow_self_signed: bool,
    /// CIDR list of proxies trusted for REST forwarded-address rate limits.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
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
            external_tls: false,
            login: super::login_listener::LoginListenerSettings::default(),
            allow_self_signed: false,
            trusted_proxies: Vec::new(),
        }
    }
}

impl HttpListenerSettings {
    fn validate_https_origin(value: &str) -> Result<(), String> {
        let authority = value
            .strip_prefix("https://")
            .ok_or_else(|| "origin must use https".to_string())?;
        if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
            return Err("origin must contain only scheme and authority".into());
        }
        if canonical_authority(authority, true)? != authority {
            return Err("origin authority is not canonical".into());
        }
        Ok(())
    }

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
            return Err(invalid(format!("HTTP head/body/request timeout must be greater than zero (effective head_timeout_ms={head_timeout_ms} ms from {}, body_timeout_ms={} ms, request_timeout_ms={} ms)", if self.head_timeout_ms.is_some() { "listener override" } else { "security.connection.auth_init_timeout_ms inheritance" }, self.body_timeout_ms, self.request_timeout_ms)));
        }
        if self.body_timeout_ms > self.request_timeout_ms
            || head_timeout_ms > self.request_timeout_ms
        {
            return Err(invalid(format!("HTTP timeout limits invalid: effective head_timeout_ms={head_timeout_ms} ms (from {}), body_timeout_ms={} ms, request_timeout_ms={} ms; head and body must not exceed request", if self.head_timeout_ms.is_some() { "listeners[].head_timeout_ms" } else { "security.connection.auth_init_timeout_ms inheritance" }, self.body_timeout_ms, self.request_timeout_ms)));
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
        if self.login.enabled {
            let invalid_login =
                |message: &str| ConfigError::Validation(format!("listeners[{idx}]: {message}"));
            if tls && self.external_tls {
                return Err(invalid_login(
                    "external_tls is invalid when the listener uses TLS",
                ));
            }
            if !tls && self.allow_plaintext_remote {
                return Err(invalid_login(
                    "login cannot be enabled with allow_plaintext_remote",
                ));
            }
            if head_timeout_ms > 5_000 {
                return Err(invalid_login(
                    "login effective head timeout must be at most 5000 ms",
                ));
            }
            if listener.kdf_override.is_some() {
                return Err(invalid_login(
                    "kdf_override is forbidden on login listeners",
                ));
            }
            if !tls && !self.external_tls && !addr.ip().is_loopback() {
                return Err(invalid_login(
                    "direct plaintext login requires a loopback bind",
                ));
            }
            if self.external_tls {
                if !addr.ip().is_loopback() || self.trusted_proxies.is_empty() {
                    return Err(invalid_login(
                        "external_tls requires loopback bind and non-empty trusted_proxies",
                    ));
                }
                if self.login.authority_allowlist.is_empty() {
                    return Err(invalid_login(
                        "external_tls requires explicit non-empty authority_allowlist",
                    ));
                }
                let mut origins: Vec<&str> = Vec::new();
                for origin in &listener.browser_origin_allowlist {
                    Self::validate_https_origin(origin).map_err(|_| {
                        invalid_login("external_tls requires canonical HTTPS origins")
                    })?;
                    if origins.contains(&origin.as_str()) {
                        return Err(invalid_login(
                            "browser_origin_allowlist contains duplicates",
                        ));
                    }
                    origins.push(origin.as_str());
                }
                if origins.is_empty() {
                    return Err(invalid_login(
                        "external_tls requires canonical HTTPS origins",
                    ));
                }
            } else if tls && !addr.ip().is_loopback() && self.login.authority_allowlist.is_empty() {
                return Err(invalid_login(
                    "direct non-loopback TLS login requires explicit authority_allowlist",
                ));
            }
            let mut canonical = Vec::new();
            for authority in &self.login.authority_allowlist {
                let value = canonical_authority(authority, tls || self.external_tls).map_err(
                    |message| invalid_login(&format!("invalid login authority: {message}")),
                )?;
                if canonical.contains(&value) {
                    return Err(invalid_login(
                        "login authority_allowlist contains canonical duplicates",
                    ));
                }
                canonical.push(value);
            }
            if let Some(id) = &self.login.listener_id {
                parse_listener_id(id)
                    .map_err(|message| invalid_login(&format!("login.listener_id {message}")))?;
            }
        }
        for proxy in &self.trusted_proxies {
            let (address, prefix) = proxy
                .split_once('/')
                .ok_or_else(|| invalid(format!("trusted_proxies entry `{proxy}` must be CIDR")))?;
            let ip: IpAddr = address.parse().map_err(|error| {
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
