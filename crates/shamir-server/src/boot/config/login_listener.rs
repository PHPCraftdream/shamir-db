use std::net::{Ipv4Addr, Ipv6Addr};

use serde::Deserialize;

/// HTTP password-login options for one listener.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct LoginListenerSettings {
    /// Enable password login routes on this listener.
    #[serde(default)]
    pub enabled: bool,
    /// Exact permitted Host authorities. Empty selects loopback aliases.
    #[serde(default)]
    pub authority_allowlist: Vec<String>,
    /// Optional stable 16-byte listener identity as 32 lowercase hex digits.
    #[serde(default)]
    pub listener_id: Option<String>,
}

/// Decode the canonical lowercase hexadecimal listener identity.
pub(super) fn parse_listener_id(value: &str) -> Result<[u8; 16], String> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("must be exactly 32 lowercase hexadecimal characters".into());
    }
    let mut id = [0u8; 16];
    for (index, byte) in id.iter_mut().enumerate() {
        let offset = index * 2;
        *byte =
            (hex_nibble(value.as_bytes()[offset]) << 4) | hex_nibble(value.as_bytes()[offset + 1]);
    }
    Ok(id)
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => 0,
    }
}

/// Canonicalize an authority and normalize its default port.
pub(super) fn canonical_authority(value: &str, tls: bool) -> Result<String, String> {
    if value.is_empty()
        || value.bytes().any(|byte| {
            byte.is_ascii_whitespace() || matches!(byte, b'/' | b'@' | b'#' | b'?' | b'\\')
        })
    {
        return Err("invalid authority syntax".into());
    }
    let default_port = if tls { 443 } else { 80 };
    if value.starts_with('[') {
        let end = value
            .find(']')
            .ok_or_else(|| "invalid bracketed IPv6 authority".to_string())?;
        let ip: Ipv6Addr = value[1..end]
            .parse()
            .map_err(|_| "invalid IPv6 address".to_string())?;
        let suffix = &value[end + 1..];
        let port = if suffix.is_empty() {
            default_port
        } else {
            parse_port(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| "invalid authority suffix".to_string())?,
            )?
        };
        if !suffix.is_empty() && !suffix.starts_with(':') {
            return Err("invalid authority suffix".into());
        }
        return Ok(if port == default_port {
            format!("[{ip}]")
        } else {
            format!("[{ip}]:{port}")
        });
    }
    let (host, port) = match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, parse_port(port)?),
        Some(_) => return Err("IPv6 authorities must be bracketed".into()),
        None => (value, default_port),
    };
    if host.parse::<Ipv4Addr>().is_ok() {
        return Ok(if port == default_port {
            host.to_owned()
        } else {
            format!("{host}:{port}")
        });
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|label| {
        label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err("invalid hostname".into());
    }
    let host = host.to_ascii_lowercase();
    Ok(if port == default_port {
        host
    } else {
        format!("{host}:{port}")
    })
}

fn parse_port(value: &str) -> Result<u16, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid port".into());
    }
    value.parse::<u16>().map_err(|_| "invalid port".into())
}
