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

use thiserror::Error;

/// Operator-configured origin policy for the browser endpoint.
#[derive(Debug, Clone)]
pub struct BrowserOriginPolicy {
    allowed: Vec<String>,
}

impl BrowserOriginPolicy {
    /// Empty allowlist — rejects everything except the explicit
    /// `accept_no_origin = true` mode (which is for testing only).
    pub fn empty() -> Self {
        Self { allowed: vec![] }
    }

    /// Construct from a list of allowed origins. Each entry can be either:
    /// - exact: `https://app.example.com`
    /// - wildcard: `https://*.example.com` (matches one subdomain level)
    pub fn allow(origins: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: origins.into_iter().map(Into::into).collect(),
        }
    }

    /// Test whether an `Origin` header value is allowed by this policy.
    pub fn allows(&self, origin: &str) -> bool {
        self.allowed
            .iter()
            .any(|pattern| origin_matches(pattern, origin))
    }

    /// Number of configured allowed origins.
    pub fn allowed_count(&self) -> usize {
        self.allowed.len()
    }
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
