use super::common::{post, Harness};
use shamir_connect::server::token::TokenMode;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_server::config::HttpListenerSettings;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_http_login_hides_both_routes() {
    let settings = HttpListenerSettings {
        tls: Some(false),
        ..Default::default()
    };
    let harness = Harness::launch_with("127.0.0.1:0", settings).await;
    let addr = harness.addr();
    for path in ["/shamir/v1/auth/begin", "/shamir/v1/auth/finish"] {
        for method in ["POST", "GET"] {
            let (status, headers, body) = if method == "POST" {
                post(addr, path, &[]).await
            } else {
                get(addr, path).await
            };
            assert_eq!(status, 404, "{method} {path}");
            assert_eq!(
                rmp_serde::from_slice::<DisabledError>(&body).expect("strict disabled error"),
                DisabledError {
                    error: "not_found".into()
                }
            );
            let headers = headers.to_ascii_lowercase();
            assert!(headers.contains("cache-control: no-store"), "{headers}");
            assert!(headers.contains("pragma: no-cache"), "{headers}");
            assert!(
                headers.contains("x-content-type-options: nosniff"),
                "{headers}"
            );
            assert!(headers.contains("connection: close"), "{headers}");
        }
    }
    let token = harness
        .token("disabled-login-ping", TokenMode::ReadOnly)
        .await;
    let ping = rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping");
    let (status, _, body) =
        super::super::http_rest::common::request(addr, Some(&token), &ping).await;
    assert_eq!(status, 200);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode ping"),
        DbResponse::Pong
    ));
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}

#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DisabledError {
    error: String,
}

async fn get(addr: std::net::SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(15), get_inner(addr, path))
        .await
        .expect("GET request timed out")
}

async fn get_inner(addr: std::net::SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .expect("write request");
    let mut response = Vec::new();
    let mut limited = (&mut stream).take(12_289);
    limited
        .read_to_end(&mut response)
        .await
        .expect("read response");
    assert!(
        response.len() <= 12_288,
        "GET response exceeds 8192-byte headers plus 4096-byte body"
    );
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response headers");
    let headers = String::from_utf8_lossy(&response[..split]).into_owned();
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("status")
        .parse()
        .expect("numeric status");
    (status, headers, response[split + 4..].to_vec())
}
