//! Ticket resume over the plain loopback TCP listener (binding 0x00), and
//! ticket mobility between plain TCP and the native TCP+TLS listener.

use std::net::SocketAddr;
use std::time::Duration;

use shamir_client::{Client, ClientError, ResumeOptions};

use crate::common::{boot, exercise, options, resume_options};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_over_plain_tcp_after_a_plain_tcp_session() {
    let matrix = boot().await;

    let first = Client::connect_plain(options(matrix.plain))
        .await
        .expect("connect_plain");
    let resumed = Client::resume_plain(resume_options(matrix.plain, &first))
        .await
        .expect("resume_plain");
    assert_eq!(resumed.server_pub_key_pin(), first.server_pub_key_pin());
    exercise(&resumed, "resumed_plain").await;

    resumed.close().await;
    first.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_ticket_resumes_over_native_tcp() {
    let matrix = boot().await;

    let plain_client = Client::connect_plain(options(matrix.plain))
        .await
        .expect("connect_plain");
    // The launcher does not disable plain-ticket upgrade, so a 0x00 ticket
    // may resume on the stronger 0x01 listener — mirrored from the TCP<->WS
    // ticket moves of the native profile.
    let resumed = Client::resume(resume_options(matrix.tcp, &plain_client))
        .await
        .expect("native resume of a plain ticket");
    resumed.ping().await.expect("ping");

    resumed.close().await;
    plain_client.close().await;
    matrix.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_tcp_ticket_is_refused_over_plain_tcp() {
    let matrix = boot().await;

    let tcp_client = Client::connect(options(matrix.tcp))
        .await
        .expect("connect tcp");
    // Anti-downgrade: a 0x01 ticket may not resume on a 0x00 listener. The
    // server rejects and closes; the client reports an error, no panic.
    let err = match Client::resume_plain(resume_options(matrix.plain, &tcp_client)).await {
        Err(e) => e,
        Ok(_) => panic!("resume downgrade 0x01 -> 0x00 must be refused"),
    };
    assert!(!err.to_string().is_empty(), "{err}");

    tcp_client.close().await;
    matrix.shutdown().await;
}

#[tokio::test]
async fn resume_plain_refuses_non_loopback_addresses_before_any_socket() {
    for text in ["192.0.2.1:9", "[2001:db8::1]:9"] {
        let addr: SocketAddr = text.parse().expect("socket addr");
        let opts = ResumeOptions {
            addr,
            server_name: "localhost".into(),
            ticket: vec![1; 16],
            pinned_hash: [7u8; 32],
        };
        let result = tokio::time::timeout(Duration::from_secs(1), Client::resume_plain(opts))
            .await
            .expect("refusal is immediate");
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("non-loopback plain TCP must be refused"),
        };
        assert!(matches!(err, ClientError::Handshake { .. }), "{err}");
        assert!(
            err.to_string().contains("plain TCP is loopback-only"),
            "{err}"
        );
    }
}
