use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use shamir_transport_http::{LoginHead, PeerInfo};

use super::Source;

/// Policy for validating login request authority, origin, and client address.
#[derive(Clone, Debug)]
pub struct LoginHeadPolicy {
    security: u8,
    authorities: Vec<String>,
    origins: Vec<String>,
    trusted: Vec<Cidr>,
}

/// Request-head validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadError {
    /// HTTP response status.
    pub status: u16,
    /// Stable error identifier.
    pub code: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct Cidr {
    address: IpAddr,
    prefix: u8,
}

impl LoginHeadPolicy {
    /// Creates a login head policy, rejecting malformed configuration.
    pub fn new(
        security: u8,
        authorities: Vec<String>,
        origins: Vec<String>,
        trusted_cidrs: Vec<String>,
        bound: SocketAddr,
    ) -> Result<Self, HeadError> {
        if security > 2 {
            return Err(invalid());
        }
        if security == 0 && !bound.ip().is_loopback() {
            return Err(invalid());
        }
        let authorities = if authorities.is_empty() && bound.ip().is_loopback() {
            let port = bound.port();
            vec![
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ]
            .into_iter()
            .map(|value| canonical_authority(&value, security))
            .collect::<Result<Vec<_>, _>>()?
        } else {
            authorities
                .into_iter()
                .map(|v| canonical_authority(&v, security))
                .collect::<Result<Vec<_>, _>>()?
        };
        let origins = origins
            .into_iter()
            .map(|v| {
                let canonical = canonical_origin(&v)?;
                if canonical != v {
                    return Err(forbidden("origin_forbidden"));
                }
                Ok(canonical)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if security == 2 && origins.iter().any(|v| !v.starts_with("https://")) {
            return Err(invalid());
        }
        let trusted = trusted_cidrs
            .into_iter()
            .map(|s| parse_cidr(&s).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            security,
            authorities,
            origins,
            trusted,
        })
    }

    /// Validates request headers and resolves the effective client source.
    pub fn resolve(&self, head: &LoginHead<'_>, peer: &PeerInfo) -> Result<Source, HeadError> {
        let mut hosts = head.header_values(b"host");
        let host = hosts.next().ok_or_else(invalid)?;
        if hosts.next().is_some() {
            return Err(invalid());
        }
        let host = std::str::from_utf8(host).map_err(|_| invalid())?;
        let authority = canonical_authority(host, self.security)?;
        if !self.authorities.iter().any(|v| v == &authority) {
            return Err(forbidden("authority_forbidden"));
        }
        let mut origins = head.header_values(b"origin");
        if let Some(value) = origins.next() {
            if origins.next().is_some() {
                return Err(forbidden("origin_forbidden"));
            }
            let origin = std::str::from_utf8(value)
                .ok()
                .and_then(|s| canonical_origin(s).ok().filter(|canonical| canonical == s));
            if !origin.is_some_and(|v| self.origins.iter().any(|a| a == &v)) {
                return Err(forbidden("origin_forbidden"));
            }
        }
        let effective = match self.security {
            0 => {
                if !peer.addr.ip().to_canonical().is_loopback() {
                    return Err(forbidden("proxy_forbidden"));
                }
                peer.addr.ip()
            }
            1 => peer.addr.ip(),
            2 => {
                if !self
                    .trusted
                    .iter()
                    .any(|cidr| contains(*cidr, peer.addr.ip()))
                {
                    return Err(forbidden("proxy_forbidden"));
                }
                let mut found = None;
                for value in peer.forwarded_for.iter().rev() {
                    for item in value.split(|b| *b == b',').rev() {
                        let item = trim_ascii(item);
                        let Some(ip) = parse_forwarded(item) else {
                            return Err(invalid());
                        };
                        if self.trusted.iter().any(|cidr| contains(*cidr, ip)) {
                            continue;
                        }
                        found = Some(ip);
                        break;
                    }
                    if found.is_some() {
                        break;
                    }
                }
                found.ok_or_else(invalid)?
            }
            _ => return Err(invalid()),
        };
        Ok(Source::from_ip(effective))
    }
}

fn invalid() -> HeadError {
    HeadError {
        status: 400,
        code: "invalid_request",
    }
}
fn forbidden(code: &'static str) -> HeadError {
    HeadError { status: 403, code }
}
fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while s.first().is_some_and(u8::is_ascii_whitespace) {
        s = &s[1..];
    }
    while s.last().is_some_and(u8::is_ascii_whitespace) {
        s = &s[..s.len() - 1];
    }
    s
}
fn parse_forwarded(bytes: &[u8]) -> Option<IpAddr> {
    let s = std::str::from_utf8(bytes).ok()?;
    if let Ok(ip) = IpAddr::from_str(s) {
        return Some(ip);
    }
    SocketAddr::from_str(s).ok().map(|a| a.ip())
}
fn canonical_authority(input: &str, security: u8) -> Result<String, HeadError> {
    if input.is_empty()
        || input.bytes().any(|b| {
            b.is_ascii_whitespace()
                || b.is_ascii_control()
                || b == b'/'
                || b == b'\\'
                || b == b'?'
                || b == b'#'
                || b == b'@'
        })
    {
        return Err(invalid());
    }
    let (host, port) = if input.starts_with('[') {
        let end = input.find(']').ok_or_else(invalid)?;
        let ip = input[1..end]
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| invalid())?;
        let tail = &input[end + 1..];
        let port = if tail.is_empty() {
            None
        } else {
            Some(parse_port(tail.strip_prefix(':').ok_or_else(invalid)?)?)
        };
        (format!("[{}]", ip), port)
    } else {
        let (h, p) = match input.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h, Some(parse_port(p)?)),
            _ => (input, None),
        };
        if h.is_empty()
            || !h
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            || h.split('.')
                .any(|label| label.is_empty() || label.starts_with('-') || label.ends_with('-'))
        {
            return Err(invalid());
        }
        (h.to_ascii_lowercase(), p)
    };
    let default = if security == 0 { 80 } else { 443 };
    Ok(match port {
        Some(p) if p != default => format!("{host}:{p}"),
        _ => host,
    })
}
fn parse_port(port: &str) -> Result<u16, HeadError> {
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    port.parse::<u16>().map_err(|_| invalid())
}
fn canonical_origin(input: &str) -> Result<String, HeadError> {
    let (scheme, remainder) = input
        .split_once("://")
        .ok_or_else(|| forbidden("origin_forbidden"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(forbidden("origin_forbidden"));
    }
    let authority = canonical_authority(remainder, if scheme == "https" { 1 } else { 0 })
        .map_err(|_| forbidden("origin_forbidden"))?;
    Ok(format!("{scheme}://{authority}"))
}
fn parse_cidr(input: &str) -> Option<Cidr> {
    let (addr, prefix) = input.split_once('/')?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let address = IpAddr::from_str(addr).ok()?;
    let bits = prefix.parse::<u8>().ok()?;
    if bits > if address.is_ipv4() { 32 } else { 128 } {
        return None;
    }
    Some(Cidr {
        address,
        prefix: bits,
    })
}
fn contains(c: Cidr, address: IpAddr) -> bool {
    let address = match address {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        IpAddr::V4(ip) => IpAddr::V4(ip),
    };
    let address = match (c.address, address) {
        (IpAddr::V6(_), IpAddr::V4(ip)) => IpAddr::V6(ip.to_ipv6_mapped()),
        (_, address) => address,
    };
    match (c.address, address) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let m = if c.prefix == 0 {
                0
            } else {
                u32::MAX.checked_shl(u32::from(32 - c.prefix)).unwrap_or(0)
            };
            u32::from(a) & m == u32::from(b) & m
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let m = if c.prefix == 0 {
                0
            } else {
                u128::MAX
                    .checked_shl(u32::from(128 - c.prefix))
                    .unwrap_or(0)
            };
            u128::from(a) & m == u128::from(b) & m
        }
        _ => false,
    }
}
