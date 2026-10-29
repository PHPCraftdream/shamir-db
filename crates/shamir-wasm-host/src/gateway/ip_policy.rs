//! Explicit egress classification for IP literals and DNS answers.

use std::net::{IpAddr, Ipv4Addr};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum IpClass {
    Global,
    Internal,
    Forbidden,
}

// Prefixes are stored as network-order integers. Matching classes combine by
// severity, except ::1, which deliberately precedes the compatible /96 rule.
const V4_PREFIXES: &[(u32, u32, IpClass)] = &[
    (0x00000000, 8, IpClass::Forbidden),
    (0xe0000000, 4, IpClass::Forbidden),
    (0xf0000000, 4, IpClass::Forbidden),
    (0x7f000000, 8, IpClass::Internal),
    (0x0a000000, 8, IpClass::Internal),
    (0xac100000, 12, IpClass::Internal),
    (0xc0a80000, 16, IpClass::Internal),
    (0xa9fe0000, 16, IpClass::Internal),
    (0x64400000, 10, IpClass::Internal),
    (0xc0000000, 24, IpClass::Internal),
    (0xc0000200, 24, IpClass::Internal),
    (0xc6336400, 24, IpClass::Internal),
    (0xcb007100, 24, IpClass::Internal),
    (0xc6120000, 15, IpClass::Internal),
    (0xc0586300, 24, IpClass::Internal),
];

const V6_PREFIXES: &[(u128, u32, IpClass)] = &[
    (0, 96, IpClass::Forbidden),
    (0xff000000000000000000000000000000, 8, IpClass::Forbidden),
    (0x01000000000000000000000000000000, 64, IpClass::Forbidden),
    (0xfe800000000000000000000000000000, 10, IpClass::Internal),
    (0xfc000000000000000000000000000000, 7, IpClass::Internal),
    (0xfec00000000000000000000000000000, 10, IpClass::Internal),
    (0x20010db8000000000000000000000000, 32, IpClass::Internal),
    (0x3fff0000000000000000000000000000, 20, IpClass::Internal),
    (0x20010000000000000000000000000000, 23, IpClass::Internal),
    (0x0064ff9b000100000000000000000000, 48, IpClass::Internal),
    (0x5f000000000000000000000000000000, 16, IpClass::Internal),
];

pub(crate) fn classify_ip(ip: IpAddr) -> IpClass {
    match ip {
        IpAddr::V4(ip) => {
            let value = u32::from(ip);
            V4_PREFIXES
                .iter()
                .filter(|(prefix, bits, _)| value >> (32 - bits) == prefix >> (32 - bits))
                .map(|(_, _, class)| *class)
                .max()
                .unwrap_or(IpClass::Global)
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback() {
                return IpClass::Internal;
            }
            let value = u128::from(ip);
            let outer = V6_PREFIXES
                .iter()
                .filter(|(prefix, bits, _)| value >> (128 - bits) == prefix >> (128 - bits))
                .map(|(_, _, class)| *class)
                .max()
                .unwrap_or(IpClass::Global);
            let octets = ip.octets();
            let embedded =
                if ip.to_ipv4_mapped().is_some() || value >> 32 == 0x0064ff9b0000000000000000 {
                    Some(Ipv4Addr::new(
                        octets[12], octets[13], octets[14], octets[15],
                    ))
                } else if value >> 112 == 0x2002 {
                    Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]))
                } else {
                    None
                };
            embedded.map_or(outer, |v4| outer.max(classify_ip(IpAddr::V4(v4))))
        }
    }
}
