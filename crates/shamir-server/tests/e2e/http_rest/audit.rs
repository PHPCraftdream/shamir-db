use super::common::{assert_pong, ping_body, request, Harness, ADMIN_PASSWORD};
use shamir_connect::server::token::TokenMode;
use shamir_server::server::{BootstrapMode, ServerLauncher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, timeout_at, Duration, Instant};
use zeroize::Zeroizing;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_failure_flood_returns_retry_after_and_shutdown_audit_summary() {
    let harness = Harness::launch().await;
    let mut limited = false;
    for _ in 0..48 {
        let (status, headers, _) =
            request(harness.addr(), Some("invalid-token"), &ping_body()).await;
        if status == 429 {
            assert!(headers.to_ascii_lowercase().contains("retry-after:"));
            limited = true;
            break;
        }
        assert_eq!(status, 401);
    }
    assert!(limited, "failure flood did not hit limiter");
    let report = harness.handle.shutdown().await;
    assert!(report.errors().is_empty(), "{report:?}");
    let entries =
        shamir_server::audit_appender::FjallAuditAppender::read_log_for_verify(harness.temp.path())
            .expect("read audit entries");
    assert!(entries
        .iter()
        .any(|entry| entry.event == "api_token_auth_failures"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_shutdown_closes_accepted_keep_alive_connection_and_restarts() {
    let harness = Harness::launch().await;
    let token = harness.token("http-shutdown", TokenMode::ReadOnly).await;
    let mut stream = TcpStream::connect(harness.addr()).await.expect("connect");
    let request = format!(
        "GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: keep-alive\r\n\r\n"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    timeout_at(deadline, stream.write_all(request.as_bytes()))
        .await
        .expect("ping request exceeded exchange deadline")
        .expect("write ping request");

    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        assert!(response.len() < 8192, "HTTP response headers too large");
        let n = timeout_at(deadline, stream.read(&mut chunk))
            .await
            .expect("HTTP response headers exceeded exchange deadline")
            .expect("read response headers");
        assert!(n > 0, "HTTP response ended before headers");
        response.extend_from_slice(&chunk[..n]);
        if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
    };
    let headers = String::from_utf8_lossy(&response[..header_end]);
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("status")
        .parse()
        .expect("numeric status");
    let content_length = headers
        .lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("numeric Content-Length")
            })
        })
        .expect("Content-Length response");
    assert!(
        content_length <= 1024 * 1024,
        "HTTP response body too large"
    );
    let body_start = header_end.checked_add(4).expect("body start overflow");
    let body_end = body_start
        .checked_add(content_length)
        .expect("HTTP response body length overflow");
    while response.len() < body_end {
        let n = timeout_at(deadline, stream.read(&mut chunk))
            .await
            .expect("HTTP response body exceeded exchange deadline")
            .expect("read response body");
        assert!(n > 0, "HTTP response body ended early");
        response.extend_from_slice(&chunk[..n]);
    }
    assert_pong(status, &response[body_start..body_end]);

    let Harness { handle, temp, .. } = harness;
    let report = timeout(Duration::from_secs(30), handle.shutdown())
        .await
        .expect("server shutdown exceeded lifecycle deadline");
    assert!(report.is_clean(), "{report:?}");
    let mut eof_probe = [0u8; 1];
    assert_eq!(
        timeout(Duration::from_secs(15), stream.read(&mut eof_probe))
            .await
            .expect("socket close exceeded lifecycle deadline")
            .expect("read socket after shutdown"),
        0,
        "accepted keep-alive socket should close on shutdown"
    );

    let restarted = ServerLauncher::new(super::common::config_for(temp.path()))
        .with_bootstrap(BootstrapMode::Password {
            username: "admin".into(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
        })
        .launch()
        .await
        .expect("same-directory restart");
    let report = restarted.shutdown().await;
    assert!(report.is_clean(), "{report:?}");
    drop(temp);
}
