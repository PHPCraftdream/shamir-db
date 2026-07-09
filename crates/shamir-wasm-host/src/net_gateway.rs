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
use std::net::{IpAddr, Ipv4Addr};

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
///
/// NOTE: this is the *string-level* check only. It does NOT resolve DNS, so a
/// hostname on the allowlist that resolves to a private IP still passes here.
/// Callers performing real network egress MUST additionally run
/// [`check_url_allowed_resolved`] (which resolves DNS and rejects
/// private/loopback resolution results) before connecting — see finding 2c.
pub fn check_url_allowed(url: &str, allowlist: &[String]) -> Result<(), String> {
    let parsed = parse_url(url)?;
    check_host_allowed(&parsed.host, allowlist)
}

/// Full egress guard for real network I/O (finding 2c / SSRF): runs the
/// string-level allowlist check AND resolves the host via DNS, rejecting if
/// ANY resolved address is a private/loopback IP unless the literal host was
/// itself an EXACT allowlist entry.
///
/// This closes the `meta.attacker.com → 169.254.169.254` bypass: an attacker
/// can put a wildcard-matched hostname on the allowlist (or a domain they
/// control) that resolves to an internal address; the string check alone
/// would pass, but the resolved IP would not.
///
/// An exact allowlist entry (e.g. an operator explicitly allowing an internal
/// `127.0.0.1` test target) is honoured — DNS resolution of a literal IP
/// yields that IP and the exact-match escape hatch in [`check_host_allowed`]
/// already applies.
pub async fn check_url_allowed_resolved(url: &str, allowlist: &[String]) -> Result<(), String> {
    let parsed = parse_url(url)?;
    // 1. String-level allowlist + SSRF-on-literal-host check.
    check_host_allowed(&parsed.host, allowlist)?;

    // 2. If the literal host is an EXACT allowlist entry, the operator has
    //    opted into it explicitly (incl. deliberately-internal targets); the
    //    string check above already enforced private-IP hygiene for it.
    if host_has_exact_match(&parsed.host, allowlist) {
        return Ok(());
    }

    // 3. Otherwise resolve DNS and reject if ANY address is private/loopback.
    //    `lookup_host` needs a port; append a dummy one.
    let addrs = tokio::net::lookup_host((parsed.host.as_str(), 0))
        .await
        .map_err(|e| format!("egress: DNS resolution failed for {}: {e}", parsed.host))?;
    for addr in addrs {
        if is_private_or_loopback_ip(addr.ip()) {
            return Err(format!(
                "egress to {} not allowed (resolves to private/loopback IP {})",
                parsed.host,
                addr.ip()
            ));
        }
    }
    Ok(())
}

/// Whether `host` is matched by at least one EXACT (non-wildcard) allowlist
/// entry.
fn host_has_exact_match(host: &str, allowlist: &[String]) -> bool {
    allowlist
        .iter()
        .any(|p| !p.contains('*') && glob_matches(p, host))
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
///
/// Recognises NON-CANONICAL IP forms (finding 2c MEDIUM sub-finding) that an
/// attacker could use to slip an internal address past a naive
/// `str::parse::<IpAddr>()` — bare decimal (`2130706433` → `127.0.0.1`), hex
/// (`0x7f000001`), and IPv4-mapped IPv6 (`::ffff:169.254.169.254`,
/// `[::ffff:a9fe:a9fe]`) — by canonicalising to an `IpAddr` first.
fn is_private_or_loopback_host(host: &str) -> bool {
    match canonicalize_ip(host) {
        Some(ip) => is_private_or_loopback_ip(ip),
        None => false,
    }
}

/// Normalise a host string that denotes an IP literal (in any of the
/// non-canonical forms browsers/curl accept) into an [`IpAddr`]. Returns
/// `None` for genuine hostnames (which are resolved via DNS elsewhere).
fn canonicalize_ip(host: &str) -> Option<IpAddr> {
    // 1. Standard dotted-quad / RFC-5952 IPv6 (also handles `::ffff:1.2.3.4`).
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(unmap_v4(ip));
    }

    // 2. Bare decimal `u32` (e.g. `2130706433`) or hex (`0x7f000001`).
    let as_u32 = if let Some(hex) = host.strip_prefix("0x").or_else(|| host.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else if host.bytes().all(|b| b.is_ascii_digit()) {
        host.parse::<u32>().ok()
    } else {
        None
    };
    if let Some(v) = as_u32 {
        return Some(IpAddr::V4(Ipv4Addr::from(v)));
    }

    None
}

/// Collapse an IPv4-mapped IPv6 address to its IPv4 form so the IPv4
/// private-range checks apply (e.g. `::ffff:169.254.169.254`).
///
/// Only the `::ffff:0:0/96` mapped range is folded — NOT the deprecated
/// IPv4-compatible range (`::a.b.c.d`), which would wrongly fold `::` / `::1`
/// (unspecified / loopback) into IPv4 forms and is handled by the IPv6 arm of
/// [`is_private_or_loopback_ip`] anyway.
fn unmap_v4(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Whether an IP address is loopback, private, or link-local.
fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    // Fold IPv4-mapped/compatible IPv6 down to IPv4 so the v4 ranges apply
    // to forms like `::ffff:169.254.169.254`.
    match unmap_v4(ip) {
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
            || v6.is_unspecified()
            // fe80::/10 (link-local) — not standard private but worth guarding.
            || (v6.segments()[0] & 0xffc0) == 0xfe80
            // fc00::/7 unique-local.
            || (v6.segments()[0] & 0xfe00) == 0xfc00
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
