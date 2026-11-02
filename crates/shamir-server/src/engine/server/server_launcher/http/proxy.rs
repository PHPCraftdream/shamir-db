use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use shamir_connect::server::token::Verified;
use shamir_transport_http::{AuthHeader, Endpoint, GateDecision, PeerInfo, TokenGate};

use crate::http_rest::RestGate;
use crate::server::boot_error::BootError;

const MAX_FORWARD_HEADER_BYTES: usize = 8 * 1024;

/// Applies trusted-proxy forwarding to the REST gate's failure limiter identity.
pub(super) struct ProxyGate {
    inner: Arc<RestGate>,
    trusted: Vec<Cidr>,
}

#[derive(Clone, Copy)]
pub(super) struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl ProxyGate {
    pub(super) fn new(inner: Arc<RestGate>, proxies: &[String]) -> Result<Self, BootError> {
        let trusted = proxies
            .iter()
            .map(|value| {
                parse_cidr(value)
                    .ok_or_else(|| BootError::Bind(format!("invalid trusted proxy CIDR: {value}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { inner, trusted })
    }

    fn resolved_peer(&self, peer: &PeerInfo) -> SocketAddr {
        resolve_peer(peer, &self.trusted)
    }
}

pub(super) fn resolve_peer(peer: &PeerInfo, trusted: &[Cidr]) -> SocketAddr {
    if !trusted.iter().any(|cidr| cidr.contains(peer.addr.ip())) {
        return peer.addr;
    }
    let mut total = 0usize;
    let mut chain = Vec::new();
    for header in &peer.forwarded_for {
        total = match total.checked_add(header.len()) {
            Some(total) if total <= MAX_FORWARD_HEADER_BYTES => total,
            _ => return peer.addr,
        };
        let Ok(value) = std::str::from_utf8(header) else {
            return peer.addr;
        };
        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return peer.addr;
            }
            let address = match parse_forwarded_ip(part) {
                Some(address) => address,
                None => return peer.addr,
            };
            chain.push(address);
        }
    }
    if chain.is_empty() {
        return peer.addr;
    }
    chain.reverse();
    for address in chain {
        if !trusted.iter().any(|cidr| cidr.contains(address)) {
            return SocketAddr::new(address, peer.addr.port());
        }
    }
    peer.addr
}

impl TokenGate for ProxyGate {
    type Grant = Verified;

    async fn authenticate<'a>(
        &'a self,
        auth: AuthHeader<'a>,
        endpoint: Endpoint,
        mut peer: PeerInfo,
    ) -> GateDecision<Self::Grant> {
        peer.addr = self.resolved_peer(&peer);
        self.inner.authenticate(auth, endpoint, peer).await
    }

    fn still_valid(&self, grant: &Self::Grant) -> bool {
        self.inner.still_valid(grant)
    }

    fn revalidate(&self, grant: &Self::Grant) -> GateDecision<()> {
        self.inner.revalidate(grant)
    }
}

fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    if let Ok(address) = value.parse::<IpAddr>() {
        return Some(address);
    }
    let address = value.parse::<SocketAddr>().ok()?;
    Some(address.ip())
}

pub(super) fn parse_cidr(value: &str) -> Option<Cidr> {
    let (ip, prefix) = value.split_once('/')?;
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let network = ip.parse::<IpAddr>().ok()?;
    let prefix = prefix.parse::<u8>().ok()?;
    match network {
        IpAddr::V4(_) if prefix <= 32 => Some(Cidr { network, prefix }),
        IpAddr::V6(_) if prefix <= 128 => Some(Cidr { network, prefix }),
        _ => None,
    }
}

impl Cidr {
    pub(super) fn contains(self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX
                        .checked_shl(32 - u32::from(self.prefix))
                        .unwrap_or(0)
                };
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX
                        .checked_shl(128 - u32::from(self.prefix))
                        .unwrap_or(0)
                };
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}
