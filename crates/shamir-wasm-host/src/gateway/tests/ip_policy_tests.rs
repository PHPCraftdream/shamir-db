//! Boundary and allowlist tests for the explicit IP policy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::gateway::ip_policy::{classify_ip, IpClass};
use crate::gateway::net_gateway::{
    check_host_allowed, check_url_allowed_resolved, check_url_allowed_resolved_with,
    NetGatewayError,
};

#[test]
fn prefix_boundaries() {
    // Network, prefix length, class, class immediately before, class after.
    let v4 = [
        (
            "0.0.0.0",
            8,
            IpClass::Forbidden,
            None,
            Some(IpClass::Global),
        ),
        (
            "224.0.0.0",
            4,
            IpClass::Forbidden,
            Some(IpClass::Global),
            Some(IpClass::Forbidden),
        ),
        (
            "240.0.0.0",
            4,
            IpClass::Forbidden,
            Some(IpClass::Forbidden),
            None,
        ),
        (
            "127.0.0.0",
            8,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "10.0.0.0",
            8,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "172.16.0.0",
            12,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "192.168.0.0",
            16,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "169.254.0.0",
            16,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "100.64.0.0",
            10,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "192.0.0.0",
            24,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "192.0.2.0",
            24,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "198.51.100.0",
            24,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "203.0.113.0",
            24,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "198.18.0.0",
            15,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
        (
            "192.88.99.0",
            24,
            IpClass::Internal,
            Some(IpClass::Global),
            Some(IpClass::Global),
        ),
    ];
    for (network, bits, class, before, after) in v4 {
        let first = u32::from(network.parse::<Ipv4Addr>().unwrap());
        let last = first | (u32::MAX >> bits);
        for value in [first, last] {
            assert_eq!(
                classify_ip(IpAddr::V4(value.into())),
                class,
                "{network}/{bits}"
            );
        }
        if let Some(expected) = before {
            assert_eq!(
                classify_ip(IpAddr::V4((first - 1).into())),
                expected,
                "before {network}/{bits}"
            );
        }
        assert_eq!(
            last.checked_add(1).is_some(),
            after.is_some(),
            "after {network}/{bits}"
        );
        if let (Some(value), Some(expected)) = (last.checked_add(1), after) {
            assert_eq!(
                classify_ip(IpAddr::V4(value.into())),
                expected,
                "after {network}/{bits}"
            );
        }
    }
    let v6 = [
        (
            "100::",
            64,
            IpClass::Forbidden,
            IpClass::Global,
            Some(IpClass::Global),
        ),
        ("ff00::", 8, IpClass::Forbidden, IpClass::Internal, None),
        (
            "fe80::",
            10,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Internal),
        ),
        (
            "fc00::",
            7,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Global),
        ),
        (
            "fec0::",
            10,
            IpClass::Internal,
            IpClass::Internal,
            Some(IpClass::Forbidden),
        ),
        (
            "2001:db8::",
            32,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Global),
        ),
        (
            "3fff::",
            20,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Global),
        ),
        (
            "2001::",
            23,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Global),
        ),
        (
            "64:ff9b:1::",
            48,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Global),
        ),
        (
            "5f00::",
            16,
            IpClass::Internal,
            IpClass::Global,
            Some(IpClass::Global),
        ),
    ];
    for (network, bits, class, before, after) in v6 {
        let first = u128::from(network.parse::<Ipv6Addr>().unwrap());
        let last = first | (u128::MAX >> bits);
        for value in [first, last] {
            assert_eq!(
                classify_ip(IpAddr::V6(value.into())),
                class,
                "{network}/{bits}"
            );
        }
        assert_eq!(
            classify_ip(IpAddr::V6((first - 1).into())),
            before,
            "before {network}/{bits}"
        );
        assert_eq!(
            last.checked_add(1).is_some(),
            after.is_some(),
            "after {network}/{bits}"
        );
        if let (Some(value), Some(expected)) = (last.checked_add(1), after) {
            assert_eq!(
                classify_ip(IpAddr::V6(value.into())),
                expected,
                "after {network}/{bits}"
            );
        }
    }
    for (ip, class) in [
        ("::", IpClass::Forbidden),
        ("::1", IpClass::Internal),
        ("::2", IpClass::Forbidden),
        ("::ffff:ffff", IpClass::Forbidden),
        ("::1:0:0", IpClass::Global),
    ] {
        assert_eq!(classify_ip(ip.parse().unwrap()), class, "{ip}");
    }
}

#[test]
fn embedded_and_match_matrix() {
    for (ip, class) in [
        ("8.8.8.8", IpClass::Global),
        ("100.64.0.1", IpClass::Internal),
        ("224.0.0.1", IpClass::Forbidden),
        ("64:ff9b::a9fe:a9fe", IpClass::Internal),
        ("64:ff9b::7f00:1", IpClass::Internal),
        ("2002:7f00:1::", IpClass::Internal),
        ("::ffff:0.0.0.0", IpClass::Forbidden),
        ("2002:e000:1::", IpClass::Forbidden),
    ] {
        assert_eq!(classify_ip(ip.parse().unwrap()), class, "{ip}");
        assert_eq!(
            check_host_allowed(ip, &["*".into()]).is_ok(),
            class == IpClass::Global,
            "{ip}"
        );
        assert_eq!(
            check_host_allowed(ip, &[ip.into()]).is_ok(),
            class != IpClass::Forbidden,
            "{ip}"
        );
    }
}

#[test]
fn embedded_prefix_boundaries() {
    // The endpoints of the embedded prefixes inherit the IPv4 policy;
    // adjacent addresses outside those prefixes retain their outer class.
    for (ip, expected) in [
        ("::fffe:ffff:ffff", IpClass::Global),
        ("::ffff:0:0", IpClass::Forbidden),
        ("::ffff:ffff:ffff", IpClass::Forbidden),
        ("::1:0:0:0", IpClass::Global),
        ("64:ff9a:ffff:ffff:ffff:ffff:ffff:ffff", IpClass::Global),
        ("64:ff9b::", IpClass::Forbidden),
        ("64:ff9b::ffff:ffff", IpClass::Forbidden),
        ("64:ff9b::1:0:0", IpClass::Global),
        ("2001:ffff:ffff:ffff:ffff:ffff:ffff:ffff", IpClass::Global),
        ("2002::", IpClass::Forbidden),
        (
            "2002:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            IpClass::Forbidden,
        ),
        ("2003::", IpClass::Global),
        ("64:ff9b:1::", IpClass::Internal),
        ("2002:0808:0808::", IpClass::Global),
        ("::ffff:8.8.8.8", IpClass::Global),
    ] {
        assert_eq!(classify_ip(ip.parse().unwrap()), expected, "{ip}");
    }
}

#[test]
fn curl_config_disables_environment_proxies() {
    // There is no configuration-builder seam in shamir-db; inspect the actual
    // initialization, rather than a duplicate of the intended config string.
    let source = include_str!("../../../../shamir-db/src/shamir_db/gateway/curl_gateway.rs");
    assert!(source.contains("let mut cfg = String::from(\"noproxy = \\\"*\\\"\\n\");"));
}

#[tokio::test]
async fn hostname_dns_answer_matrix() {
    let answers = [
        (
            "global",
            vec!["8.8.8.8", "1.1.1.1", "8.8.8.8"],
            [true, true, true],
        ),
        ("internal", vec!["127.0.0.1"], [true, false, false]),
        ("forbidden", vec!["224.0.0.1"], [false, false, false]),
        (
            "global_internal",
            vec!["8.8.8.8", "10.0.0.1"],
            [true, false, false],
        ),
        (
            "internal_global",
            vec!["10.0.0.1", "8.8.8.8"],
            [true, false, false],
        ),
        (
            "global_forbidden",
            vec!["8.8.8.8", "224.0.0.1"],
            [false, false, false],
        ),
        (
            "forbidden_global",
            vec!["224.0.0.1", "8.8.8.8"],
            [false, false, false],
        ),
    ];
    for (label, ips, allowed) in answers {
        for (index, pattern) in ["service.example.com", "*.example.com", "*"]
            .iter()
            .enumerate()
        {
            let addresses: Vec<std::net::SocketAddr> = ips
                .iter()
                .map(|ip| std::net::SocketAddr::new(ip.parse().unwrap(), 8443))
                .collect();
            let result = check_url_allowed_resolved_with(
                "https://service.example.com:8443/",
                &[(*pattern).into()],
                |host, port| async move {
                    assert_eq!(host, "service.example.com");
                    assert_eq!(port, 8443);
                    Ok(addresses)
                },
            )
            .await;
            if allowed[index] {
                let pin = result.unwrap();
                let mut expected = Vec::new();
                for ip in &ips {
                    let ip = ip.parse::<IpAddr>().unwrap();
                    if !expected.contains(&ip) {
                        expected.push(ip);
                    }
                }
                assert_eq!(pin.host, "service.example.com", "{label} {pattern}");
                assert_eq!(pin.port, 8443, "{label} {pattern}");
                assert_eq!(pin.pinned_ips, expected, "{label} {pattern}");
            } else {
                assert!(
                    matches!(result, Err(NetGatewayError::NotAllowed(_))),
                    "{label} {pattern}"
                );
            }
        }
    }
}

#[tokio::test]
async fn exact_hostname_dns_failure_is_reported() {
    let error = check_url_allowed_resolved("http://bad\0name/", &["bad\0name".into()])
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::gateway::net_gateway::NetGatewayError::Dns { .. }
    ));
}

#[tokio::test]
async fn exact_hostname_resolves_and_is_pinned() {
    let pin = check_url_allowed_resolved("http://localhost/", &["localhost".into()])
        .await
        .unwrap();
    assert!(!pin.pinned_ips.is_empty());
    assert!(pin.pinned_ips.iter().all(|ip| ip.is_loopback()));
}
