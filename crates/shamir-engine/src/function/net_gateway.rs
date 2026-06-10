//! Network egress gateway trait for WASM function HTTP access (slice 8c).
//!
//! A [`NetGateway`] is the async boundary between the WASM host-import layer
//! and the outside world. Functions call `ctx.http_fetch(req)` which bottoms
//! out in one of these trait methods.
//!
//! # Security: default-deny allowlist + SSRF guard
//!
//! The gateway holds an allowlist of host patterns. Before any outbound
//! request the guard checks:
//!
//! 1. **Scheme** — only `http` and `https` are permitted.
//! 2. **Allowlist match** — the host must match at least one allowlist entry.
//! 3. **SSRF hygiene** — loopback / private IPs (`127.0.0.0/8`, `::1`,
//!    `10/8`, `172.16/12`, `192.168/16`, `169.254/16`) are permitted ONLY
//!    by an EXACT (non-wildcard) allowlist entry. A wildcard like
//!    `*.example.com` must NOT cover a private IP that happens to resolve
//!    from that domain. This lets tests allow `127.0.0.1` explicitly while
//!    preventing a wildcard from being tricked into hitting internal IPs.

use async_trait::async_trait;
use std::net::IpAddr;

/// An HTTP request to send through the gateway.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// HTTP method (`GET`, `POST`, etc.).
    pub method: String,
    /// Request URL.
    pub url: String,
    /// Request headers as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// Request body (empty for GET/HEAD).
    pub body: Vec<u8>,
}

/// An HTTP response from the gateway.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code (e.g. 200, 404).
    pub status: u16,
    /// Response headers as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: Vec<u8>,
}

/// Async gateway that lets a WASM function make outbound HTTP requests.
///
/// Implemented once in the `shamir-db` crate (`CurlNetGateway`) by wrapping
/// the system `curl` binary. The WASM host-import layer holds an
/// `Option<Arc<dyn NetGateway>>` — `None` means egress was not configured
/// for this invocation, and the `http_fetch` host import traps.
#[async_trait]
pub trait NetGateway: Send + Sync {
    /// Execute the given HTTP request and return the response.
    ///
    /// The implementation MUST run the allowlist / SSRF guard before
    /// performing any network I/O.
    async fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, String>;
}

// ── SSRF guard ───────────────────────────────────────────────────────

/// Check whether a host is permitted by the allowlist, applying SSRF
/// hygiene rules.
///
/// Returns `Ok(())` if permitted, `Err(reason)` otherwise.
pub fn check_host_allowed(host: &str, allowlist: &[String]) -> Result<(), String> {
    if allowlist.is_empty() {
        return Err(format!("egress to {host} not allowed"));
    }

    // Find the best matching allowlist entry.
    let mut matched_wildcard = false;
    let mut matched_exact = false;

    for pattern in allowlist {
        if glob_matches(pattern, host) {
            if pattern.contains('*') {
                matched_wildcard = true;
            } else {
                matched_exact = true;
            }
        }
    }

    if !matched_wildcard && !matched_exact {
        return Err(format!("egress to {host} not allowed"));
    }

    // SSRF hygiene: if the host looks like a loopback / private IP,
    // it must be matched by an EXACT (non-wildcard) entry.
    if is_private_or_loopback_host(host) && !matched_exact {
        return Err(format!(
            "egress to {host} not allowed (private/loopback IP requires exact allowlist entry)"
        ));
    }

    Ok(())
}

/// Parse a URL and check scheme + host against the allowlist.
pub fn check_url_allowed(url: &str, allowlist: &[String]) -> Result<(), String> {
    let parsed = parse_url(url)?;
    check_host_allowed(&parsed.host, allowlist)
}

/// Parsed URL components relevant to the guard.
pub(crate) struct ParsedUrl {
    pub(crate) host: String,
}

/// Minimal URL parser — extract scheme and host. Does not depend on the
/// `url` crate (keeps the binary lean).
pub(crate) fn parse_url(url: &str) -> Result<ParsedUrl, String> {
    // Find "://"
    let scheme_end = url
        .find("://")
        .ok_or_else(|| format!("egress: invalid URL (no scheme): {url}"))?;
    let scheme = &url[..scheme_end];
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "egress: scheme '{scheme}' not allowed (only http/https)"
        ));
    }

    let rest = &url[scheme_end + 3..];
    // Host runs until '/', '?', '#', or end.
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_end];

    // Strip userinfo (user:pass@) if present.
    let host_port = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };

    // Strip bracket-enclosed IPv6.
    let host = if host_port.starts_with('[') {
        // IPv6: [::1]:port
        let close = host_port
            .find(']')
            .ok_or_else(|| format!("egress: invalid IPv6 host in URL: {url}"))?;
        host_port[1..close].to_string()
    } else {
        // IPv4 or hostname — strip :port
        match host_port.rfind(':') {
            Some(colon) => host_port[..colon].to_string(),
            None => host_port.to_string(),
        }
    };

    if host.is_empty() {
        return Err(format!("egress: empty host in URL: {url}"));
    }

    Ok(ParsedUrl { host })
}

/// Whether the host string looks like a loopback or private IP address.
fn is_private_or_loopback_host(host: &str) -> bool {
    match host.parse::<IpAddr>() {
        Ok(ip) => is_private_or_loopback_ip(ip),
        Err(_) => false,
    }
}

/// Whether an IP address is loopback, private, or link-local.
fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 127.0.0.0/8
            octets[0] == 127
            // 10.0.0.0/8
            || octets[0] == 10
            // 172.16.0.0/12
            || (octets[0] == 172 && (octets[1] & 0xf0) == 16)
            // 192.168.0.0/16
            || (octets[0] == 192 && octets[1] == 168)
            // 169.254.0.0/16
            || (octets[0] == 169 && octets[1] == 254)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
            // fe80::/10 (link-local) — not standard private but worth guarding.
            || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Tiny `*`-only glob matcher — reuses the same logic as `EnvPolicy`.
///
/// A pattern is split at `*` into literal segments that must appear
/// consecutively in `text`. A single `*` matches zero or more characters.
fn glob_matches(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.is_empty() {
        return true;
    }
    let mut cursor = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[cursor..].find(part) {
            Some(pos) => {
                if i == 0 && !pattern.starts_with('*') && pos != 0 {
                    return false;
                }
                cursor += pos + part.len();
            }
            None => return false,
        }
    }
    if !pattern.ends_with('*') && cursor != text.len() {
        return false;
    }
    true
}
