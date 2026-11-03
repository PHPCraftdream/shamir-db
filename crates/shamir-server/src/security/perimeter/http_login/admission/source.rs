use std::net::IpAddr;

/// Canonical network prefix identifying one source hierarchy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PrefixKey {
    family: u8,
    prefix: u8,
    bytes: [u8; 16],
}
impl PrefixKey {
    /// Address family tag (4 or 6).
    pub const fn family(self) -> u8 {
        self.family
    }
    /// Prefix length in bits.
    pub const fn prefix(self) -> u8 {
        self.prefix
    }
    /// Network bytes, zero-padded to 16 bytes.
    pub const fn bytes(self) -> [u8; 16] {
        self.bytes
    }
}

/// Normalized source address; prefix identities are derived on demand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Source {
    address: IpAddr,
}
impl Source {
    /// Canonicalizes IPv4-mapped IPv6 addresses.
    pub fn from_ip(address: IpAddr) -> Self {
        Self {
            address: match address {
                IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
                IpAddr::V4(ip) => IpAddr::V4(ip),
            },
        }
    }
    /// Normalized effective address.
    pub const fn address(self) -> IpAddr {
        self.address
    }
    /// Returns policy tiers from narrowest to broadest.
    pub fn tiers(self) -> [PrefixKey; 3] {
        match self.address {
            IpAddr::V4(ip) => {
                let b = ip.octets();
                [v4_key(b, 24), v4_key(b, 16), v4_key(b, 16)]
            }
            IpAddr::V6(ip) => {
                let b = ip.octets();
                [v6_key(b, 64), v6_key(b, 56), v6_key(b, 48)]
            }
        }
    }
    /// Returns the fair-share principal (/16 or /48).
    pub fn principal(self) -> PrefixKey {
        match self.address {
            IpAddr::V4(ip) => v4_key(ip.octets(), 16),
            IpAddr::V6(ip) => v6_key(ip.octets(), 48),
        }
    }
}
fn v4_key(octets: [u8; 4], prefix: u8) -> PrefixKey {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&octets);
    mask(&mut bytes, prefix);
    PrefixKey {
        family: 4,
        prefix,
        bytes,
    }
}
fn v6_key(mut bytes: [u8; 16], prefix: u8) -> PrefixKey {
    mask(&mut bytes, prefix);
    PrefixKey {
        family: 6,
        prefix,
        bytes,
    }
}
fn mask(bytes: &mut [u8; 16], prefix: u8) {
    let whole = usize::from(prefix / 8);
    let rem = prefix % 8;
    if rem != 0 {
        bytes[whole] &= u8::MAX << (8 - rem);
    }
    bytes[whole + usize::from(rem != 0)..].fill(0);
}
