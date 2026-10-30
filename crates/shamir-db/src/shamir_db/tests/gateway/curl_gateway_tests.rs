use crate::shamir_db::curl_gateway::{
    build_resolve_lines, escape_curl_value, CurlNetGateway, LifecycleEvent,
};
use shamir_engine::function::ResolvedPin;
use shamir_engine::function::{HttpRequest, NetGateway, NetGatewayError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZeroUsize;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DEADLINE: Duration = Duration::from_secs(15);

async fn read_request_headers(stream: &mut tokio::net::TcpStream) {
    const MAX_HEADERS: usize = 16 * 1024;
    let mut headers = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let remaining = MAX_HEADERS - headers.len();
        assert!(remaining > 0, "request headers exceed {MAX_HEADERS} bytes");
        let n = stream
            .read(&mut chunk[..remaining.min(1024)])
            .await
            .expect("request header read failed");
        assert!(n > 0, "EOF before end of request headers");
        headers.extend_from_slice(&chunk[..n]);
        if headers.windows(4).any(|window| window == b"\r\n\r\n") {
            return;
        }
    }
}

fn request(addr: std::net::SocketAddr) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: format!("http://{addr}/"),
        headers: vec![],
        body: vec![],
    }
}

async fn spawned(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<LifecycleEvent>,
) -> std::path::PathBuf {
    match tokio::time::timeout(DEADLINE, events.recv())
        .await
        .expect("timed out waiting for curl spawn")
    {
        Some(LifecycleEvent::Spawned { pid, path }) => {
            assert_ne!(pid, 0);
            path
        }
        Some(LifecycleEvent::SpawnFailed(error)) => panic!("curl spawn failed: {error}"),
        other => panic!("expected curl spawn, got {other:?}"),
    }
}

async fn completed(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<LifecycleEvent>,
    expected: &std::path::Path,
) {
    match tokio::time::timeout(DEADLINE, events.recv())
        .await
        .expect("timed out waiting for curl cleanup")
    {
        Some(LifecycleEvent::Completed { path, wait }) => {
            assert_eq!(path, expected);
            assert!(wait.is_ok(), "curl wait/reap failed: {wait:?}");
            assert!(!path.exists(), "curl temp directory still exists: {path:?}");
        }
        other => panic!("expected curl cleanup, got {other:?}"),
    }
}

fn assert_transport_contains(
    result: Result<shamir_engine::function::HttpResponse, NetGatewayError>,
    expected: &str,
) {
    match result {
        Err(NetGatewayError::Transport(message)) => assert!(
            message.contains(expected),
            "unexpected transport error: {message}"
        ),
        Err(error) => panic!("expected transport error containing {expected:?}, got {error:?}"),
        Ok(_) => panic!("expected transport error containing {expected:?}, got success"),
    }
}

async fn serve_response(
    response: &'static [u8],
    limit: usize,
) -> Result<shamir_engine::function::HttpResponse, NetGatewayError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(DEADLINE, async {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream.write_all(response).await.unwrap();
        })
        .await
        .expect("server timed out accepting, reading or writing");
    });
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let gateway = CurlNetGateway::with_response_limit(
        vec!["127.0.0.1".into()],
        NonZeroUsize::new(limit).unwrap(),
    )
    .with_observer(tx);
    let result = tokio::time::timeout(DEADLINE, gateway.fetch(request(addr)))
        .await
        .expect("curl request timed out");
    let path = spawned(&mut events).await;
    completed(&mut events, &path).await;
    tokio::time::timeout(DEADLINE, server)
        .await
        .expect("server join timed out")
        .expect("server failed");
    result
}

#[tokio::test]
async fn cancellation_reaps_curl_and_removes_tempdir() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        tokio::time::timeout(DEADLINE, async {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            let _ = accepted_tx.send(());
            let _ = release_rx.await;
        })
        .await
        .expect("cancellation server timed out");
    });
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let gateway = CurlNetGateway::new(vec!["127.0.0.1".into()]).with_observer(tx);
    let fetch = tokio::spawn(async move { gateway.fetch(request(addr)).await });
    let path = spawned(&mut events).await;
    tokio::time::timeout(DEADLINE, accepted_rx)
        .await
        .expect("server accept timed out")
        .expect("server failed before accept");
    fetch.abort();
    assert!(tokio::time::timeout(DEADLINE, fetch)
        .await
        .expect("fetch abort timed out")
        .unwrap_err()
        .is_cancelled());
    completed(&mut events, &path).await;
    let _ = release_tx.send(());
    tokio::time::timeout(DEADLINE, server)
        .await
        .expect("server join timed out")
        .expect("server failed");
}

#[tokio::test]
async fn default_limit_rejects_16_mib_plus_one() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(DEADLINE, async {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 16777217\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let chunk = [b'x'; 8192];
            for _ in 0..2048 {
                if stream.write_all(&chunk).await.is_err() {
                    return;
                }
            }
            let _ = stream.write_all(b"x").await;
        })
        .await
        .expect("default-limit server timed out");
    });
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let gateway = CurlNetGateway::new(vec!["127.0.0.1".into()]).with_observer(tx);
    let result = tokio::time::timeout(DEADLINE, gateway.fetch(request(addr)))
        .await
        .expect("default-limit request timed out");
    let path = spawned(&mut events).await;
    completed(&mut events, &path).await;
    assert_transport_contains(result, "response body exceeds 16777216 bytes");
    tokio::time::timeout(DEADLINE, server)
        .await
        .expect("server join timed out")
        .expect("server failed");
}

#[test]
fn response_limit_defaults_and_explicit_positive_value() {
    assert_eq!(
        CurlNetGateway::new(vec![]).response_limit(),
        16 * 1024 * 1024
    );
    assert_eq!(
        CurlNetGateway::with_response_limit(vec![], NonZeroUsize::new(7).unwrap()).response_limit(),
        7
    );
}

#[tokio::test]
async fn response_headers_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("headers");
    let prefix = b"HTTP/1.1 200 OK\r\nX-Pad: ";
    let mut bytes = prefix.to_vec();
    bytes.resize(1024 * 1024, b'a');
    tokio::fs::write(&path, &bytes).await.unwrap();
    assert_eq!(
        crate::shamir_db::curl_gateway::parse_response_headers(&path)
            .await
            .unwrap()
            .0,
        200
    );
    bytes.push(b'a');
    tokio::fs::write(&path, bytes).await.unwrap();
    match crate::shamir_db::curl_gateway::parse_response_headers(&path).await {
        Err(NetGatewayError::Transport(message)) => {
            assert!(message.contains("response headers exceed 1048576 bytes"))
        }
        other => panic!("expected header limit error, got {other:?}"),
    }
}

#[tokio::test]
async fn response_body_exact_limit_succeeds() {
    let response = serve_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd",
        4,
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"abcd");
}

#[tokio::test]
async fn response_body_over_limit_is_transport_error() {
    assert_transport_contains(
        serve_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde",
            4,
        )
        .await,
        "response body exceeds 4 bytes",
    );
}

#[tokio::test]
async fn chunked_body_over_limit_is_transport_error() {
    assert_transport_contains(serve_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n", 4).await, "response body exceeds 4 bytes");
}

#[tokio::test]
async fn chunked_oversize_is_rejected_before_end_of_stream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        tokio::time::timeout(DEADLINE, async {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_headers(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n")
                .await
                .unwrap();
            let _ = sent_tx.send(());
            let _ = release_rx.await;
        })
        .await
        .expect("chunked server timed out");
    });
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let gateway = CurlNetGateway::with_response_limit(
        vec!["127.0.0.1".into()],
        NonZeroUsize::new(4).unwrap(),
    )
    .with_observer(tx);
    let fetch = tokio::spawn(async move { gateway.fetch(request(addr)).await });
    let path = spawned(&mut events).await;
    tokio::time::timeout(DEADLINE, sent_rx)
        .await
        .expect("chunk send timed out")
        .expect("server failed before sending chunk");
    let result = tokio::time::timeout(DEADLINE, fetch)
        .await
        .expect("fetch did not reject before server release")
        .expect("fetch task failed");
    assert_transport_contains(result, "response body exceeds 4 bytes");
    completed(&mut events, &path).await;
    release_tx.send(()).expect("server exited before release");
    tokio::time::timeout(DEADLINE, server)
        .await
        .expect("server join timed out")
        .expect("server failed");
}

#[tokio::test]
async fn incomplete_transfer_is_transport_error() {
    assert_transport_contains(
        serve_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabcd",
            8,
        )
        .await,
        "curl failed",
    );
}

#[test]
fn escape_curl_value_backslash() {
    assert_eq!(escape_curl_value(r"a\b"), r"a\\b");
}

#[test]
fn escape_curl_value_quotes() {
    assert_eq!(escape_curl_value(r#"a"b"#), r#"a\"b"#);
}

#[test]
fn escape_curl_value_plain() {
    assert_eq!(escape_curl_value("hello"), "hello");
}

// ── Finding 2c: DNS-rebind pin wired into curl.cfg ───────────────────────

#[test]
fn resolve_line_pins_single_validated_ip() {
    // The curl config must carry `resolve = "host:port:ip"` with EXACTLY the
    // IP the SSRF guard validated, so curl connects to that address instead of
    // re-resolving the hostname (DNS-rebind window closed).
    let pin = ResolvedPin {
        host: "api.example.com".to_string(),
        port: 443,
        pinned_ips: vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
    };
    let lines = build_resolve_lines(&pin);
    assert_eq!(lines, "resolve = \"api.example.com:443:93.184.216.34\"\n");
}

#[test]
fn resolve_line_pins_all_validated_ips() {
    // When DNS returned several addresses (e.g. A + AAAA), pin ALL of them so
    // curl can only ever connect to a guard-validated address.
    let pin = ResolvedPin {
        host: "example.com".to_string(),
        port: 80,
        pinned_ips: vec![
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        ],
    };
    let lines = build_resolve_lines(&pin);
    assert_eq!(
        lines,
        "resolve = \"example.com:80:203.0.113.5,[2001:db8::1]\"\n"
    );
}

#[test]
fn resolve_line_empty_for_exact_allowlist_path() {
    // Exact-allowlist-match path performs no DNS resolution → empty pin set →
    // no `resolve` line is emitted (curl resolves the operator-allowed host).
    let pin = ResolvedPin {
        host: "127.0.0.1".to_string(),
        port: 8080,
        pinned_ips: vec![],
    };
    assert_eq!(build_resolve_lines(&pin), "");
}
