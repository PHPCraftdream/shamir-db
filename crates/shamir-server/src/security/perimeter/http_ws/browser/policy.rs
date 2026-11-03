use std::net::Ipv6Addr;

/// Returns the canonical HTTP(S) tuple origin, or `None` when invalid.
pub(crate) fn valid_origin(value: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(value).ok()?;
    if value == "null"
        || value.is_empty()
        || value
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return None;
    }
    let (scheme, authority) = value.split_once("://")?;
    if !matches!(scheme, "http" | "https")
        || authority.is_empty()
        || authority.contains('/')
        || authority.contains(['?', '#', '@'])
    {
        return None;
    }
    let tls = scheme == "https";
    let canonical = canonical_authority(authority, tls)?;
    Some(format!("{scheme}://{canonical}"))
}

/// Verifies a raw Host value against an already validated listener allowlist.
pub(crate) fn host_allowed(value: &[u8], allowlist: &[String], tls: bool) -> bool {
    let Ok(value) = std::str::from_utf8(value) else {
        return false;
    };
    canonical_authority(value, tls)
        .is_some_and(|canonical| allowlist.iter().any(|entry| entry == &canonical))
}

/// Returns the canonical authority for a browser WebSocket listener entry.
/// Entries must already be canonical: host names are lowercase, IPv6 literals
/// are bracketed, and the scheme-default port is omitted.
pub(crate) fn canonical_authority(value: &str, tls: bool) -> Option<String> {
    if value.is_empty()
        || value
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        || value.contains(['@', '/', '?', '#', '%'])
    {
        return None;
    }
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let (literal, suffix) = rest.split_once(']')?;
        let address = literal.parse::<Ipv6Addr>().ok()?;
        (format!("[{address}]"), parse_port(suffix)?)
    } else {
        let (host, suffix) = match value.find(':') {
            Some(at) => value.split_at(at),
            None => (value, ""),
        };
        let host = host.to_ascii_lowercase();
        let label_ok = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        if host.len() > 253 || !host.split('.').all(label_ok) {
            return None;
        }
        (host, parse_port(suffix)?)
    };
    let default = if tls { 443 } else { 80 };
    Some(match port.filter(|port| *port != default) {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// `""` -> no port; `":N"` with 1 <= N <= 65535 -> `Some(N)`; anything else invalid.
fn parse_port(suffix: &str) -> Option<Option<u16>> {
    if suffix.is_empty() {
        return Some(None);
    }
    let digits = suffix.strip_prefix(':')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .map(Some)
}
