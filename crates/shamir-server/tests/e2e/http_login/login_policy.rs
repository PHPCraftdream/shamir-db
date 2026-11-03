use super::common::{begin_body_for, login_as, post, proof_for, PASSWORD, USERNAME};
use shamir_connect::server::token::TokenMode;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_server::bootstrap::{ensure_superuser, BootstrapPolicy};
use shamir_server::config::HttpListenerSettings;
use shamir_server::user_directory::FjallUserDirectory;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_http_login_hides_both_routes() {
    let settings = HttpListenerSettings {
        tls: Some(false),
        ..Default::default()
    };
    let harness = super::common::Harness::launch_with("127.0.0.1:0", settings).await;
    for path in ["/shamir/v1/auth/begin", "/shamir/v1/auth/finish"] {
        for method in ["POST", "GET"] {
            let (status, headers, body) = if method == "POST" {
                post(harness.addr(), path, &[]).await
            } else {
                get(harness.addr(), path).await
            };
            assert_eq!(status, 404);
            let error: DisabledError = rmp_serde::from_slice(&body).expect("decode disabled error");
            assert_eq!(
                error,
                DisabledError {
                    error: "not_found".into()
                }
            );
            let headers = headers.to_ascii_lowercase();
            for header in [
                "cache-control: no-store",
                "pragma: no-cache",
                "x-content-type-options: nosniff",
                "connection: close",
            ] {
                assert!(headers.contains(header), "missing {header}: {headers}");
            }
        }
    }
    let token = harness
        .token("disabled-login-ping", TokenMode::ReadOnly)
        .await;
    let ping = rmp_serde::to_vec_named(&DbRequest::Ping).expect("encode ping");
    let (status, _, body) =
        crate::http_rest::common::request(harness.addr(), Some(&token), &ping).await;
    assert_eq!(status, 200);
    assert!(matches!(
        rmp_serde::from_slice::<DbResponse>(&body).expect("decode ping response"),
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
        .expect("HTTP request timed out")
}

async fn get_inner(addr: std::net::SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect HTTP listener");
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write HTTP request");
    let mut response = Vec::new();
    let mut limited = (&mut stream).take(12_289);
    limited
        .read_to_end(&mut response)
        .await
        .expect("read HTTP response");
    assert!(response.len() <= 12_288, "HTTP response too large");
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP headers");
    let headers = String::from_utf8_lossy(&response[..header_end]).into_owned();
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("status")
        .parse()
        .expect("numeric status");
    (status, headers, response[header_end + 4..].to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_login_read_write_is_issued_when_enabled() {
    let nonce = [9; 32];
    let harness = super::common::launch_with_options(false, true).await;
    let (status, _, body) = post(
        harness.addr(),
        "/shamir/v1/auth/begin",
        &begin_body_for(USERNAME, "e2e-write", "read_write", &nonce),
    )
    .await;
    assert_eq!(status, 200, "enabled read-write policy allows a challenge");
    let challenge: super::common::LoginChallenge =
        rmp_serde::from_slice(&body).expect("decode read-write challenge");
    let handshake_id: [u8; 32] = challenge
        .handshake_id
        .as_ref()
        .try_into()
        .expect("handshake ID");
    let finish = super::common::finish_body(
        &handshake_id,
        &proof_for(USERNAME, "e2e-write", "read_write", PASSWORD, &challenge),
    );
    let (status, _, response) = post(harness.addr(), "/shamir/v1/auth/finish", &finish).await;
    assert_eq!(
        status, 200,
        "read-write finish issues a real token when globally enabled"
    );
    let issued: super::common::LoginSuccess =
        rmp_serde::from_slice(&response).expect("decode read-write token");
    assert_eq!(issued.mode, "read_write");
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_login_superuser_receives_documented_account_denial() {
    let harness = super::common::Harness::launch_with_hooks(
        "127.0.0.1:0",
        HttpListenerSettings {
            tls: Some(false),
            login: Default::default(),
            ..Default::default()
        },
        true,
        |config| config.listeners[0].http.login.enabled = true,
        |path| {
            let directory = FjallUserDirectory::open(path.join("users")).expect("user directory");
            ensure_superuser(
                &directory,
                path,
                "eligible-root",
                BootstrapPolicy::Password(b"eligible-root-password"),
                &shamir_connect::common::kdf_params::KdfParams {
                    memory_kb: 19_456,
                    time: 2,
                    parallelism: 1,
                    argon2_version: 0x13,
                },
            )
            .expect("seed superuser");
        },
    )
    .await;
    let result = login_as(
        harness.addr(),
        "eligible-root",
        "eligible-superuser",
        b"eligible-root-password",
    )
    .await;
    let (status, _, body) = result.err().expect("superuser cannot receive HTTP token");
    assert_eq!(status, 403);
    let error: Denial = rmp_serde::from_slice(&body).expect("decode account denial");
    assert_eq!(error.error, "login_account_denied");
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Denial {
    error: String,
}
