//! Hostname connect regressions (RV-CLIENT-NODE-8FB7D9BF).

use std::net::{IpAddr, Ipv6Addr};
use std::time::Duration;

use shamir_client::resolve_host;
use shamir_client::{Client, ClientError, ConnectOptions};
use zeroize::Zeroizing;

/// A hostname (not an IP literal) must be DNS-resolved, not rejected.
#[tokio::test]
async fn rv_client_node_8fb7d9bf_hostname_is_resolved_not_parsed() {
    let addr = resolve_host("localhost", 3742)
        .await
        .expect("localhost must resolve on every supported platform");
    assert!(addr.ip().is_loopback(), "expected loopback, got {addr}");
    assert_eq!(addr.port(), 3742);
}

/// An RFC 2606/6761-reserved name never resolves: the failure must be the
/// resolver's I/O error, not the old `invalid host:port` parse error.
#[tokio::test]
async fn rv_client_node_8fb7d9bf_unresolvable_hostname_yields_resolver_error() {
    let err = resolve_host("shamir-db.invalid", 1)
        .await
        .expect_err("reserved TLD must not resolve");
    assert!(
        matches!(err, shamir_client::ClientError::Io(_)),
        "unexpected: {err:?}"
    );
}

/// IPv6 literals — bare and bracketed — must construct a `SocketAddr`
/// without DNS (the pre-fix `format!("{}:{}", host, port)` path could not
/// parse a bare IPv6 literal at all).
#[tokio::test]
async fn rv_client_node_8fb7d9bf_ipv6_literals_construct_socket_addr() {
    let bare = resolve_host("::1", 3742).await.expect("bare IPv6 literal");
    assert_eq!(bare.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
    let bracketed = resolve_host("[::1]", 3742)
        .await
        .expect("bracketed IPv6 literal");
    assert_eq!(bracketed.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
}

/// The full connect sequence the Node binding performs — `resolve_host` for
/// the hostname, then the PUBLIC `Client::connect` with the resolved
/// address — must reach the TCP/TLS handshake phase (the peer closes
/// immediately, so the failure is a transport-phase error, never an
/// address-parse error).
#[tokio::test]
async fn rv_client_node_8fb7d9bf_connect_entry_reaches_tcp_via_hostname() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let port = listener.local_addr().expect("listener addr").port();
    // Peer: accept, then immediately drop — the handshake must fail at the
    // transport phase, proving the hostname reached a real socket address.
    tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.expect("accept");
    });

    let addr = resolve_host("localhost", port)
        .await
        .expect("localhost must resolve");
    assert_eq!(addr.port(), port);

    let opts = ConnectOptions {
        addr,
        server_name: "localhost".into(),
        username: "host-probe".into(),
        password: Zeroizing::new(b"unused".to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(Duration::from_secs(5)),
        handshake_timeout: Some(Duration::from_secs(5)),
        request_timeout: None,
    };
    match Client::connect(opts).await {
        Err(
            ClientError::Tls { .. }
            | ClientError::Io(_)
            | ClientError::Transport { .. }
            | ClientError::Handshake { .. },
        ) => {}
        Err(other) => panic!("expected a transport-phase error, got {other:?}"),
        Ok(_) => panic!("a peer that closes immediately cannot complete a handshake"),
    }
}

/// Discriminator (RV-CLIENT-NODE-8FB7D9BF): `Client::connect_host` is THE
/// production hostname path — the Node binding calls it directly and does
/// no parsing of its own — so restoring a `SocketAddr::parse`-style
/// shortcut here means NO connection is ever attempted and the server's
/// accept below is never observed, failing this test. Every resolved
/// address is tried, so an IPv6-first `localhost` resolution cannot make
/// the test pass by refusal against an IPv4-only listener.
#[tokio::test]
async fn rv_client_node_8fb7d9bf_connect_host_reaches_local_listener() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let port = listener.local_addr().expect("listener addr").port();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.expect("accept");
        let _ = accepted_tx.send(());
        // Hold the socket open without speaking: the handshake must fail at
        // the transport phase — never at address parsing.
        std::future::pending::<()>().await;
    });

    let opts = ConnectOptions {
        // Overwritten by connect_host from host:port — never used.
        addr: "0.0.0.0:0".parse().expect("placeholder addr"),
        server_name: "localhost".into(),
        username: "host-probe".into(),
        password: Zeroizing::new(b"unused".to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(Duration::from_secs(2)),
        handshake_timeout: Some(Duration::from_secs(2)),
        request_timeout: None,
    };
    let raced = tokio::time::timeout(
        Duration::from_secs(30),
        Client::connect_host("localhost", port, opts),
    )
    .await;
    match raced.expect("connect_host must stay bounded") {
        Err(
            ClientError::Tls { .. }
            | ClientError::Io(_)
            | ClientError::Transport { .. }
            | ClientError::Handshake { .. }
            | ClientError::HandshakeTimeout(..),
        ) => {}
        Err(other) => panic!("expected a transport-phase error, got {other:?}"),
        Ok(_) => panic!("a silent peer cannot complete a handshake"),
    }

    // THE discriminator: the listener must have ACTUALLY accepted the
    // connection this call made.
    tokio::time::timeout(Duration::from_secs(5), accepted_rx)
        .await
        .expect("server accept must be observed promptly")
        .expect("accept channel open");
    server.abort();
}
