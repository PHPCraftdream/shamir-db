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

/// The outcome of a successful [`check_url_allowed_resolved`] check: the exact
/// host/port/IP(s) the guard validated, so the actual network call can be
/// PINNED to them and cannot be re-resolved to a different address at
/// connection time (finding 2c DNS-rebind TOCTOU fix).
#[derive(Debug, Clone)]
pub struct ResolvedPin {
    /// The literal host from the URL authority (sent as `Host`/SNI).
    pub host: String,
    /// The connection port (explicit or scheme-defaulted).
    pub port: u16,
    /// The validated IP(s) to pin curl's connection to via
    /// `--resolve host:port:ip`. EMPTY means "do not pin": this is the
    /// exact-allowlist-match path, where no DNS resolution happened (the
    /// operator opted into the literal host explicitly), so there is no
    /// rebind window to close and pinning would be meaningless.
    pub pinned_ips: Vec<IpAddr>,
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
///
/// # DNS-rebind TOCTOU (finding 2c)
///
/// Returns the validated [`ResolvedPin`] so the caller can pin its actual
/// connection to the SAME address(es) the guard checked. Without this, the
/// guard's resolution and curl's connection-time resolution are two
/// independent DNS lookups: an attacker controlling authoritative DNS can
/// answer "safe public IP" for the first and "internal IP" for the second.
/// Pinning via curl `--resolve host:port:ip` removes curl's second lookup
/// entirely, so what was validated is exactly what is connected to.
pub async fn check_url_allowed_resolved(
    url: &str,
    allowlist: &[String],
) -> Result<ResolvedPin, String> {
    let parsed = parse_url(url)?;
    // 1. String-level allowlist + SSRF-on-literal-host check.
    check_host_allowed(&parsed.host, allowlist)?;

    // 2. If the literal host is an EXACT allowlist entry, the operator has
    //    opted into it explicitly (incl. deliberately-internal targets); the
    //    string check above already enforced private-IP hygiene for it. No DNS
    //    resolution happened, so there is nothing to pin (empty `pinned_ips`).
    if host_has_exact_match(&parsed.host, allowlist) {
        return Ok(ResolvedPin {
            host: parsed.host,
            port: parsed.port,
            pinned_ips: Vec::new(),
        });
    }

    // 3. Otherwise resolve DNS and reject if ANY address is private/loopback.
    //    `lookup_host` needs a port; use the real one so the pin matches.
    let addrs: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((parsed.host.as_str(), parsed.port))
            .await
            .map_err(|e| format!("egress: DNS resolution failed for {}: {e}", parsed.host))?
            .collect();
    let mut pinned_ips: Vec<IpAddr> = Vec::with_capacity(addrs.len());
    for addr in &addrs {
        if is_private_or_loopback_ip(addr.ip()) {
            return Err(format!(
                "egress to {} not allowed (resolves to private/loopback IP {})",
                parsed.host,
                addr.ip()
            ));
        }
        // Dedup while preserving resolution order.
        if !pinned_ips.contains(&addr.ip()) {
            pinned_ips.push(addr.ip());
        }
    }
    if pinned_ips.is_empty() {
        return Err(format!(
            "egress: DNS resolution for {} returned no addresses",
            parsed.host
        ));
    }
    Ok(ResolvedPin {
        host: parsed.host,
        port: parsed.port,
        pinned_ips,
    })
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
    /// The connection port, taken from the authority when present or defaulted
    /// from the scheme (80 for http, 443 for https). Needed to build curl's
    /// `--resolve host:port:ip` pin (finding 2c DNS-rebind fix).
    pub(crate) port: u16,
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
    let default_port: u16 = if scheme == "https" { 443 } else { 80 };

    let rest = &url[scheme_end + 3..];
    // Host runs until '/', '?', '#', or end.
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_end];

    // Strip userinfo (user:pass@) if present.
    let host_port = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };

    // Strip bracket-enclosed IPv6, capturing an explicit port if present.
    let (host, port_str) = if host_port.starts_with('[') {
        // IPv6: [::1]:port
        let close = host_port
            .find(']')
            .ok_or_else(|| format!("egress: invalid IPv6 host in URL: {url}"))?;
        let after = &host_port[close + 1..];
        let port = after.strip_prefix(':');
        (host_port[1..close].to_string(), port)
    } else {
        // IPv4 or hostname — split off :port.
        match host_port.rfind(':') {
            Some(colon) => (
                host_port[..colon].to_string(),
                Some(&host_port[colon + 1..]),
            ),
            None => (host_port.to_string(), None),
        }
    };

    if host.is_empty() {
        return Err(format!("egress: empty host in URL: {url}"));
    }

    let port = match port_str {
        Some(p) if !p.is_empty() => p
            .parse::<u16>()
            .map_err(|_| format!("egress: invalid port in URL: {url}"))?,
        _ => default_port,
    };

    Ok(ParsedUrl { host, port })
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
pub(crate) fn canonicalize_ip(host: &str) -> Option<IpAddr> {
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

    // 3. Classic BSD `inet_aton`-compatible dotted forms that libc/curl still
    //    accept and that bypass a naive `Ipv4Addr::from_str` allowlist check:
    //      * octal per-octet   — `0177.0.0.1`   (0177 octal = 127) → 127.0.0.1
    //      * hex   per-octet   — `0x7f.0.0.1`                       → 127.0.0.1
    //      * short/shorthand   — `127.1` → 127.0.0.1, `192.168.1` → 192.168.0.1
    //    See [`parse_inet_aton`] for the exact rules.
    if let Some(v) = parse_inet_aton(host) {
        return Some(IpAddr::V4(Ipv4Addr::from(v)));
    }

    None
}

/// Parse an IPv4 address string using classic BSD `inet_aton` semantics and
/// return it as a big-endian `u32`. Returns `None` if the string is not a
/// valid `inet_aton` form.
///
/// `inet_aton` accepts 1 to 4 dot-separated numeric components; each component
/// may be written in decimal, octal (leading `0`, e.g. `0177`), or hex (leading
/// `0x`/`0X`). The number of components determines how the 32-bit address is
/// packed — the LAST component absorbs all bytes not consumed by the leading
/// single-byte components:
///
/// | form        | packing                                             |
/// |-------------|-----------------------------------------------------|
/// | `a`         | `a`               — a is the whole 32-bit value      |
/// | `a.b`       | `a<<24  | b`       — b is the low 24 bits (`≤ 2^24-1`)|
/// | `a.b.c`     | `a<<24 | b<<16 | c`— c is the low 16 bits (`≤ 2^16-1`)|
/// | `a.b.c.d`   | `a<<24 | b<<16 | c<<8 | d` — each octet `≤ 255`      |
///
/// The bare 1-component case is already handled by step 2 of
/// [`canonicalize_ip`]; it is included here for completeness/correctness so the
/// helper matches `inet_aton` exactly, but the dotted (2–4 component) forms are
/// the ones step 2 misses.
fn parse_inet_aton(host: &str) -> Option<u32> {
    // Reject empty and reject a trailing dot (`127.0.0.1.` is NOT valid).
    if host.is_empty() || host.ends_with('.') || host.starts_with('.') {
        return None;
    }

    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }

    // Parse every component as a u64 first (a lone component can be up to
    // 2^32-1; intermediate components up to 255 — bound-checked below).
    let mut vals: Vec<u64> = Vec::with_capacity(parts.len());
    for part in &parts {
        vals.push(parse_inet_component(part)?);
    }

    let n = vals.len();
    // Each leading component (all but the last) occupies exactly one byte.
    for &v in &vals[..n - 1] {
        if v > 0xff {
            return None;
        }
    }

    // The last component absorbs the remaining bytes: (5 - n) bytes worth.
    // n=1 → 32 bits, n=2 → 24 bits, n=3 → 16 bits, n=4 → 8 bits.
    let remaining_bytes = 4 - (n - 1);
    let max_last: u64 = match remaining_bytes {
        4 => u32::MAX as u64,
        3 => 0x00ff_ffff,
        2 => 0x0000_ffff,
        1 => 0x0000_00ff,
        _ => unreachable!("n is 1..=4 so remaining_bytes is 1..=4"),
    };
    let last = vals[n - 1];
    if last > max_last {
        return None;
    }

    // Pack: leading components into their high bytes, last absorbs the low bytes.
    let mut result: u32 = last as u32;
    for (i, &v) in vals[..n - 1].iter().enumerate() {
        // Leading component i sits at byte position (3 - i) from the top.
        let shift = 8 * (3 - i as u32);
        result |= (v as u32) << shift;
    }
    Some(result)
}

/// Parse a single `inet_aton` numeric component: hex (`0x`/`0X` prefix), octal
/// (leading `0`), or decimal. Returns `None` on any malformed component
/// (empty, non-digit, or an invalid digit for the detected radix).
fn parse_inet_component(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        // Hex must have at least one digit and only hex digits.
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return u64::from_str_radix(hex, 16).ok();
    }
    // Leading `0` (and length > 1) → octal. A lone `0` is decimal zero.
    if part.len() > 1 && part.starts_with('0') {
        let oct = &part[1..];
        if !oct.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return None;
        }
        return u64::from_str_radix(oct, 8).ok();
    }
    // Decimal.
    if !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
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
