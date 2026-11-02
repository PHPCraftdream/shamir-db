//! Browser WebSocket origin, authority, and pre-authentication policy.

/// Returns true only when the raw Origin has a valid serialized HTTP(S) tuple origin.
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
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || value.contains(['@', '/', '?', '#', '%'])
    {
        return None;
    }

    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let suffix = &rest[end + 1..];
        let address = host.parse::<std::net::Ipv6Addr>().ok()?;
        if address.to_string() != host.to_ascii_lowercase() {
            return None;
        }
        let port = if suffix.is_empty() {
            None
        } else {
            Some(suffix.strip_prefix(':')?.parse::<u16>().ok()?)
        };
        (format!("[{host}]"), port)
    } else {
        if value.matches(':').count() > 1 {
            return None;
        }
        let (host, port) = match value.rsplit_once(':') {
            Some((host, port)) => (host, Some(port.parse::<u16>().ok()?)),
            None => (value, None),
        };
        if host.is_empty()
            || host.starts_with('.')
            || host.ends_with('.')
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            || host != host.to_ascii_lowercase()
        {
            return None;
        }
        (host.to_string(), port)
    };

    if port == Some(if tls { 443 } else { 80 }) {
        return None;
    }
    Some(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}
