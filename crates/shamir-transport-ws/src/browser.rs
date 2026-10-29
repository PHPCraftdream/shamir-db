//! Browser-WSS endpoint policy (spec TRANSPORT_WS §9).
//!
//! Per spec the `/shamir/v1/browser` endpoint MUST validate the `Origin`
//! HTTP header on the WebSocket upgrade request. Browsers always send
//! `Origin`; native (non-browser) clients typically don't. This is the
//! primary defence against cross-site WebSocket hijacking — without it,
//! a malicious origin could open a WS to the server using the browser's
//! ambient credentials (cookies, etc.) and proxy SCRAM messages.
//!
//! ## Policy
//!
//! - Reject upgrades that **lack** an `Origin` header (spec §9: browser
//!   endpoint REQUIRES the header — native clients should use the
//!   `/shamir/v1` endpoint instead).
//! - Reject if `Origin` is not in the operator-configured allowlist.
//! - Allowlist supports exact match (`https://app.example.com`) and
//!   wildcard for subdomains (`https://*.example.com`).

use std::net::{Ipv4Addr, Ipv6Addr};
use thiserror::Error;

/// Operator-configured origin policy for the browser endpoint.
#[derive(Debug, Clone)]
pub struct BrowserOriginPolicy {
    allowed: Vec<String>,
}

impl BrowserOriginPolicy {
    /// Empty allowlist — rejects every origin, including missing headers.
    pub fn empty() -> Self {
        Self { allowed: vec![] }
    }

    /// Construct from a list of allowed origins. Each entry can be either:
    /// - exact: `https://app.example.com`
    /// - wildcard: `https://*.example.com` (matches one subdomain level)
    ///
    /// Malformed entries are ignored. Prefer `try_allow` for configuration.
    pub fn allow(origins: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: origins
                .into_iter()
                .map(Into::into)
                .filter(|s| valid_origin(s, true))
                .collect(),
        }
    }

    /// Reject malformed origin patterns instead of silently ignoring them.
    pub fn try_allow(origins: impl IntoIterator<Item = impl Into<String>>) -> Result<Self, String> {
        let allowed: Vec<String> = origins.into_iter().map(Into::into).collect();
        if let Some(bad) = allowed.iter().find(|s| !valid_origin(s, true)) {
            return Err(format!("invalid origin pattern: {bad}"));
        }
        Ok(Self { allowed })
    }

    /// Test whether an `Origin` header value is allowed by this policy.
    pub fn allows(&self, origin: &str) -> bool {
        valid_origin(origin, false)
            && self
                .allowed
                .iter()
                .any(|pattern| origin_matches(pattern, origin))
    }

    /// Number of configured allowed origins.
    pub fn allowed_count(&self) -> usize {
        self.allowed.len()
    }
}

// Validate only serialized HTTP(S) tuple origins; preserve their exact spelling.
fn valid_origin(value: &str, pattern: bool) -> bool {
    let authority = match value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    {
        Some(authority) => authority,
        None => return false,
    };
    if !authority.is_ascii()
        || authority
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b"/@?#,\\".contains(&b))
    {
        return false;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let Some((ip, suffix)) = rest.split_once(']') else {
            return false;
        };
        if ip.parse::<Ipv6Addr>().is_err() {
            return false;
        }
        ("", suffix)
    } else {
        match authority.split_once(':') {
            Some((host, _)) => (host, &authority[host.len()..]),
            None => (authority, ""),
        }
    };
    if !port.is_empty() {
        let Some(port) = port.strip_prefix(':') else {
            return false;
        };
        if port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || port.parse::<u16>().is_err()
        {
            return false;
        }
    }
    if authority.starts_with('[') {
        return true;
    }
    let (host, wildcard) = match host.strip_prefix("*.") {
        Some(host) if pattern => (host, true),
        _ => (host, false),
    };
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if !wildcard && host.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    if wildcard && host.parse::<Ipv4Addr>().is_ok() {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

fn origin_matches(pattern: &str, origin: &str) -> bool {
    // Wildcard: pattern of form `<scheme>://*.<rest>`. Match exactly one
    // subdomain component before the suffix.
    if let Some(idx) = pattern.find("//*.") {
        let scheme_prefix = &pattern[..idx + 2]; // e.g. "https://"
        let suffix = &pattern[idx + 4..]; // e.g. "example.com"
        if !origin.starts_with(scheme_prefix) {
            return false;
        }
        let after_scheme = &origin[scheme_prefix.len()..];
        // Must have exactly one component before the suffix.
        if let Some(dot_idx) = after_scheme.find('.') {
            // The part after the first dot must equal the suffix.
            let after_first_dot = &after_scheme[dot_idx + 1..];
            return after_first_dot == suffix && !after_scheme.starts_with('.');
        }
        return false;
    }
    // Exact match.
    pattern == origin
}

/// Errors raised by [`validate_origin`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum OriginRejected {
    /// Browser endpoint hit without `Origin` header — likely a native
    /// client that should use `/shamir/v1` instead.
    #[error("browser endpoint requires Origin header")]
    Missing,
    /// Origin present but not in the allowlist.
    #[error("origin not allowed: {0}")]
    NotAllowed(String),
}

/// Validate the `Origin` header against `policy`. Returns `Ok(())` if the
/// origin is allowed, or [`OriginRejected`] otherwise.
///
/// `origin_header` is the value from the WebSocket upgrade request's
/// `Origin` header (None = header missing).
pub fn validate_origin(
    policy: &BrowserOriginPolicy,
    origin_header: Option<&str>,
) -> Result<(), OriginRejected> {
    let origin = origin_header.ok_or(OriginRejected::Missing)?;
    if policy.allows(origin) {
        Ok(())
    } else {
        Err(OriginRejected::NotAllowed(origin.to_string()))
    }
}
