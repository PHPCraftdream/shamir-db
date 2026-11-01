use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::{
    serve, AuthHeader, BodyBudget, Endpoint, EndpointSet, ExecOutcome, ExecStatus, GateDecision,
    HttpLimits, HttpServerConfig, PeerInfo, RestExecutor, TokenGate,
};

#[derive(Debug, Eq, PartialEq)]
struct AuthRecord {
    value: AuthValue,
    forwarded_for: Vec<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
enum AuthValue {
    Missing,
    Single(Vec<u8>),
    Multiple,
}

struct Gate {
    calls: AtomicUsize,
    auth: std::sync::Mutex<Vec<AuthRecord>>,
    decision: GateDecision<u8>,
    valid: bool,
}
impl TokenGate for Gate {
    type Grant = u8;
    fn authenticate<'a>(
        &'a self,
        auth: AuthHeader<'a>,
        _endpoint: Endpoint,
        peer: PeerInfo,
    ) -> impl Future<Output = GateDecision<u8>> + Send + 'a {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let auth = match auth {
            AuthHeader::Missing => AuthValue::Missing,
            AuthHeader::Single(value) => AuthValue::Single(value.to_vec()),
            AuthHeader::Multiple => AuthValue::Multiple,
        };
        self.auth.lock().expect("auth lock").push(AuthRecord {
            value: auth,
            forwarded_for: peer.forwarded_for,
        });
        async move {
            match &self.decision {
                GateDecision::Allow(v) => GateDecision::Allow(*v),
                GateDecision::Unauthorized => GateDecision::Unauthorized,
                GateDecision::Forbidden(v) => GateDecision::Forbidden(v.clone()),
                GateDecision::RateLimited { retry_after_secs } => GateDecision::RateLimited {
                    retry_after_secs: *retry_after_secs,
                },
                GateDecision::Unavailable { retry_after_secs } => GateDecision::Unavailable {
                    retry_after_secs: *retry_after_secs,
                },
            }
        }
    }
    fn still_valid(&self, _: &u8) -> bool {
        self.valid
    }
}

struct Executor {
    calls: AtomicUsize,
    bodies: std::sync::Mutex<Vec<Bytes>>,
}
impl RestExecutor<u8> for Executor {
    fn execute<'a>(
        &'a self,
        _: &'a u8,
        body: Bytes,
    ) -> impl Future<Output = ExecOutcome> + Send + 'a {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.bodies.lock().expect("bodies lock").push(body);
        async {
            ExecOutcome {
                status: ExecStatus::Ok,
                body: Bytes::from_static(b"ok"),
                hold: None,
            }
        }
    }
}
struct Admission;
impl crate::ConnAdmission for Admission {
    fn admit(&self, _: &SocketAddr) -> Option<Box<dyn Send + Sync>> {
        Some(Box::new(()))
    }
}

struct Harness {
    stream: TcpStream,
    gate: Arc<Gate>,
    exec: Arc<Executor>,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), crate::ServeError>>,
    budget: Arc<BodyBudget>,
}

async fn setup(decision: GateDecision<u8>) -> Harness {
    setup_with_body_timeout(decision, Duration::from_secs(30)).await
}

async fn setup_with_body_timeout(decision: GateDecision<u8>, body_timeout: Duration) -> Harness {
    setup_with_limits(decision, body_timeout, 8192).await
}

async fn setup_with_limits(
    decision: GateDecision<u8>,
    body_timeout: Duration,
    max_head_bytes: usize,
) -> Harness {
    setup_with_validity(decision, body_timeout, max_head_bytes, true).await
}

async fn setup_with_validity(
    decision: GateDecision<u8>,
    body_timeout: Duration,
    max_head_bytes: usize,
    valid: bool,
) -> Harness {
    setup_with_validity_and_budget(decision, body_timeout, max_head_bytes, valid, 4).await
}

async fn setup_with_validity_and_budget(
    decision: GateDecision<u8>,
    body_timeout: Duration,
    max_head_bytes: usize,
    valid: bool,
    budget_cap: usize,
) -> Harness {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let gate = Arc::new(Gate {
        calls: AtomicUsize::new(0),
        auth: std::sync::Mutex::new(Vec::new()),
        decision,
        valid,
    });
    let exec = Arc::new(Executor {
        calls: AtomicUsize::new(0),
        bodies: std::sync::Mutex::new(Vec::new()),
    });
    let budget = Arc::new(BodyBudget::new(budget_cap));
    let cfg = Arc::new(HttpServerConfig {
        limits: {
            let mut limits = HttpLimits::new(1024, budget.clone());
            limits.max_head_bytes = max_head_bytes;
            limits.body_timeout = body_timeout;
            limits
        },
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::from_static(b"ping"),
    });
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        cfg,
        gate.clone(),
        exec.clone(),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let stream = TcpStream::connect(addr).await.expect("connect");
    Harness {
        stream,
        gate,
        exec,
        shutdown,
        server,
        budget,
    }
}

async fn read_response(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let byte = stream.read_u8().await.expect("read response head byte");
        head.push(byte);
    }
    let head = String::from_utf8(head).expect("HTTP headers");
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream
        .read_exact(&mut body)
        .await
        .expect("read complete response body");
    (head, body)
}

async fn assert_closed(stream: &mut TcpStream) {
    let mut extra = [0; 1];
    match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut extra))
        .await
        .expect("connection close")
    {
        Ok(0) => {}
        Ok(n) => panic!("unexpected bytes after response: {n}"),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            ) => {}
        Err(error) => panic!("unexpected close error: {error}"),
    }
}

async fn finish(harness: Harness) {
    harness.shutdown.cancel();
    harness
        .server
        .await
        .expect("server task")
        .expect("serve result");
}

#[tokio::test]
async fn rejected_request_authenticates_before_reading_body() {
    let mut harness = setup(GateDecision::Unauthorized).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 100\r\n\r\n").await.expect("write head");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 401"));
    assert_eq!(
        body,
        &[
            0x81, 0xa5, b'e', b'r', b'r', b'o', b'r', 0xac, b'u', b'n', b'a', b'u', b't', b'h',
            b'o', b'r', b'i', b'z', b'e', b'd'
        ]
    );
    assert_eq!(harness.gate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness.gate.auth.lock().expect("auth lock").as_slice(),
        &[AuthRecord {
            value: AuthValue::Missing,
            forwarded_for: Vec::new()
        }]
    );
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn rejected_expect_request_returns_fixed_401_without_continue_and_closes() {
    let mut harness = setup(GateDecision::Unauthorized).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nAuthorization: Bearer invalid\r\nExpect: 100-continue\r\nContent-Length: 100\r\n\r\n").await.expect("write head");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("final response");
    assert!(head.starts_with("HTTP/1.1 401"));
    assert!(!head.contains("100 Continue"));
    assert!(head
        .to_ascii_lowercase()
        .contains("www-authenticate: bearer"));
    assert_eq!(
        body,
        &[
            0x81, 0xa5, b'e', b'r', b'r', b'o', b'r', 0xac, b'u', b'n', b'a', b'u', b't', b'h',
            b'o', b'r', b'i', b'z', b'e', b'd'
        ]
    );
    let mut extra = [0; 1];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), harness.stream.read(&mut extra))
            .await
            .expect("close")
            .expect("read"),
        0
    );
    assert_eq!(
        harness.gate.auth.lock().expect("auth lock").as_slice(),
        &[AuthRecord {
            value: AuthValue::Single(b"Bearer invalid".to_vec()),
            forwarded_for: Vec::new()
        }]
    );
    finish(harness).await;
}

#[tokio::test]
async fn exact_content_length_budget_is_accepted_and_payload_forwarded() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\nfour").await.expect("write request");
    let (head, _) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(
        harness.exec.bodies.lock().expect("bodies lock").as_slice(),
        &[Bytes::from_static(b"four")]
    );
    assert_eq!(harness.budget.in_use(), 0);
    finish(harness).await;
}

#[tokio::test]
async fn chunked_multichunk_body_grows_budget_and_forwards_payload() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n").await.expect("write first chunk");
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness.budget.in_use() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first chunk reservation");
    harness
        .stream
        .write_all(b"2\r\ncd\r\n0\r\n\r\n")
        .await
        .expect("write second chunk");
    let (head, _) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(
        harness.exec.bodies.lock().expect("bodies lock").as_slice(),
        &[Bytes::from_static(b"abcd")]
    );
    assert_eq!(harness.budget.in_use(), 0);
    finish(harness).await;
}

#[tokio::test]
async fn bodyless_ping_uses_ping_payload_and_empty_post_stays_empty() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness
        .stream
        .write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("write ping");
    let (ping_head, ping_body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("ping response");
    assert!(ping_head.starts_with("HTTP/1.1 200"));
    assert_eq!(ping_body, b"ok");
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n").await.expect("write empty post");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(body, b"ok");
    assert_eq!(
        harness.exec.bodies.lock().expect("bodies lock").as_slice(),
        &[Bytes::from_static(b"ping"), Bytes::new()]
    );
    assert_eq!(harness.gate.calls.load(Ordering::SeqCst), 2);
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 2);
    finish(harness).await;
}

#[tokio::test]
async fn bodyless_ping_with_body_still_uses_configured_ping_payload() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness
        .stream
        .write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\n\r\nx")
        .await
        .expect("write ping body");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("ping response");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(body, b"ok");
    assert_eq!(
        harness.exec.bodies.lock().expect("bodies lock").as_slice(),
        &[Bytes::from_static(b"ping")]
    );
    finish(harness).await;
}

#[tokio::test]
async fn accepted_request_emits_security_headers_and_runs_executor() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n").await.expect("write");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(body, b"ok");
    assert!(head
        .to_ascii_lowercase()
        .contains("cache-control: no-store"));
    assert!(head
        .to_ascii_lowercase()
        .contains("x-content-type-options: nosniff"));
    assert!(!head.to_ascii_lowercase().contains("access-control-"));
    assert_eq!(harness.gate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 1);
    finish(harness).await;
}

#[tokio::test]
async fn oversized_declared_body_returns_fixed_413_without_executor() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 2048\r\n\r\n").await.expect("write head");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 413"));
    assert!(body.is_empty());
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn chunked_body_budget_n_plus_one_returns_503_and_releases_partial_reservation() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n").await.expect("write first chunk");
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness.budget.in_use() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first chunk reservation");
    harness
        .stream
        .write_all(b"3\r\ncde\r\n0\r\n\r\n")
        .await
        .expect("write over-limit chunk");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 503"));
    assert!(body.is_empty());
    assert_eq!(harness.budget.in_use(), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn body_budget_exhaustion_returns_503_and_releases_reservation() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    let held = harness.budget.try_reserve(4).expect("fill budget");
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 1\r\n\r\nx").await.expect("write request");
    let (head, _) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 503"));
    drop(held);
    assert_eq!(harness.budget.in_use(), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test(start_paused = true)]
async fn incomplete_body_timeout_releases_declared_reservation() {
    let mut harness = setup_with_body_timeout(GateDecision::Allow(7), Duration::ZERO).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\nab").await.expect("partial body");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("timeout response");
    assert!(head.starts_with("HTTP/1.1 408"));
    assert!(body.is_empty());
    assert_eq!(harness.budget.in_use(), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn allowed_expect_sends_continue_before_body_and_then_forwards_it() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nAuthorization: Bearer allowed\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\n").await.expect("write headers");
    let (interim, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("continue response");
    assert!(interim.starts_with("HTTP/1.1 100"));
    assert!(body.is_empty());
    harness.stream.write_all(b"data").await.expect("write body");
    let (final_head, final_body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("final response");
    assert!(final_head.starts_with("HTTP/1.1 200"));
    assert_eq!(final_body, b"ok");
    assert_eq!(
        harness.exec.bodies.lock().expect("bodies lock").as_slice(),
        &[Bytes::from_static(b"data")]
    );
    finish(harness).await;
}

#[tokio::test]
async fn framing_rejection_returns_fixed_431_before_authentication() {
    let mut harness = setup_with_limits(GateDecision::Allow(7), Duration::from_secs(30), 128).await;
    let mut request = b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n".to_vec();
    request.extend(vec![b'X'; 150]);
    request.extend_from_slice(b"\r\n\r\n");
    harness
        .stream
        .write_all(&request)
        .await
        .expect("write oversized head");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 431"));
    assert!(body.is_empty());
    assert!(head.to_ascii_lowercase().contains("connection: close"));
    assert_eq!(harness.gate.calls.load(Ordering::SeqCst), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn every_invalid_pipelined_head_is_canned_after_first_response() {
    let mut cases = vec![
        (b"GET /shamir/v1/ping HTTP/1.1\n\n".to_vec(), "400"),
        (b"GET /shamir/v1/ping HTTP/1.1\rHost: x\r\n\r\n".to_vec(), "400"),
        (b"GET /shamir/v1/ping HTTP/1.1\r\nX: bare\rvalue\r\n\r\n".to_vec(), "400"),
        (b"GET /shamir/v1/ping HTTP/1.1\r\n folded: x\r\n\r\n".to_vec(), "400"),
        (b"POST /shamir/v1/request HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n".to_vec(), "400"),
        (b"POST /shamir/v1/request HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(), "400"),
        (b"POST /shamir/v1/request HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n".to_vec(), "400"),
        (b"POST /shamir/v1/request HTTP/1.1\r\nContent-Length: +1\r\n\r\n".to_vec(), "400"),
        (b"POST /shamir/v1/request HTTP/1.1\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(), "400"),
        (b"GET /shamir/v1/ping HTTP/1.1\r\nX: one\0two\r\n\r\n".to_vec(), "400"),
        (b"GET /shamir/v1/ping HTTP/1.1\r\nX: one\x01two\r\n\r\n".to_vec(), "400"),
    ];
    let mut oversized = b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n".to_vec();
    oversized.extend(vec![b'X'; 150]);
    oversized.extend_from_slice(b"\r\n\r\n");
    cases.push((oversized, "431"));

    for (invalid, status) in cases {
        let first_request = b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let first_request_len = first_request.len();
        for split in 0..=invalid.len() {
            let mut harness =
                setup_with_limits(GateDecision::Allow(7), Duration::from_secs(30), 128).await;
            let mut pipeline = first_request.to_vec();
            pipeline.extend_from_slice(&invalid);
            harness
                .stream
                .write_all(&pipeline[..first_request_len + split])
                .await
                .expect("write first request and invalid prefix");
            let (first_head, first_body) =
                tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
                    .await
                    .expect("first response before invalid suffix");
            assert!(first_head.starts_with("HTTP/1.1 200 OK"));
            assert_eq!(first_body, b"ok");
            if let Err(error) = harness.stream.write_all(&invalid[split..]).await {
                assert!(
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::NotConnected
                    ),
                    "split {split}: {error}"
                );
            }
            let (second_head, second_body) =
                tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
                    .await
                    .expect("canned framing response");
            assert!(
                second_head.starts_with(&format!("HTTP/1.1 {status}")),
                "split {split}: {second_head}"
            );
            assert!(second_body.is_empty());
            let lower = second_head.to_ascii_lowercase();
            assert!(lower.contains("cache-control: no-store"));
            assert!(lower.contains("x-content-type-options: nosniff"));
            assert!(lower.contains("content-type: application/msgpack"));
            assert!(lower.contains("connection: close"));
            assert_eq!(
                harness.gate.calls.load(Ordering::SeqCst),
                1,
                "split {split}"
            );
            assert_eq!(
                harness.exec.calls.load(Ordering::SeqCst),
                1,
                "split {split}"
            );
            assert_closed(&mut harness.stream).await;
            finish(harness).await;
        }
    }
}
#[tokio::test]
async fn declared_body_limit_matrix_accepts_exact_and_rejects_one_over_early() {
    for length in [1024usize, 1025] {
        let mut harness = setup_with_validity_and_budget(
            GateDecision::Allow(7),
            Duration::from_secs(30),
            8192,
            true,
            4096,
        )
        .await;
        let mut request = format!("POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: {length}\r\n\r\n").into_bytes();
        if length == 1024 {
            request.extend(std::iter::repeat_n(b'x', length));
        }
        harness
            .stream
            .write_all(&request)
            .await
            .expect("write request");
        let (head, body) =
            tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
                .await
                .expect("response");
        if length == 1024 {
            assert!(head.starts_with("HTTP/1.1 200"));
            assert_eq!(body, b"ok");
            assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                harness.exec.bodies.lock().expect("bodies lock").as_slice(),
                &[Bytes::from(vec![b'x'; length])]
            );
        } else {
            assert!(head.starts_with("HTTP/1.1 413"));
            assert!(body.is_empty());
            assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
            assert_closed(&mut harness.stream).await;
        }
        assert_eq!(harness.budget.in_use(), 0);
        finish(harness).await;
    }

    let mut harness = setup_with_validity_and_budget(
        GateDecision::Allow(7),
        Duration::from_secs(30),
        8192,
        true,
        0,
    )
    .await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 1024\r\n\r\n").await.expect("write head");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 503"));
    assert!(body.is_empty());
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
    assert_eq!(harness.budget.in_use(), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn chunked_body_limit_matrix_accepts_exact_and_rejects_one_over() {
    for length in [1024usize, 1025] {
        let mut harness = setup_with_validity_and_budget(
            GateDecision::Allow(7),
            Duration::from_secs(30),
            8192,
            true,
            4096,
        )
        .await;
        harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nTransfer-Encoding: chunked\r\n\r\n").await.expect("write head");
        let mut framed = Vec::new();
        for remaining in [length.min(512), length.saturating_sub(512)] {
            if remaining == 0 {
                continue;
            }
            framed.extend_from_slice(format!("{remaining:x}\r\n").as_bytes());
            framed.extend(std::iter::repeat_n(b'x', remaining));
            framed.extend_from_slice(b"\r\n");
        }
        framed.extend_from_slice(b"0\r\n\r\n");
        harness
            .stream
            .write_all(&framed)
            .await
            .expect("write chunks");
        let (head, body) =
            tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
                .await
                .expect("response");
        if length == 1024 {
            assert!(head.starts_with("HTTP/1.1 200"));
            assert_eq!(body, b"ok");
            assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                harness.exec.bodies.lock().expect("bodies lock").as_slice(),
                &[Bytes::from(vec![b'x'; length])]
            );
        } else {
            assert!(head.starts_with("HTTP/1.1 413"));
            assert!(body.is_empty());
            assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
            assert_closed(&mut harness.stream).await;
        }
        assert_eq!(harness.budget.in_use(), 0);
        finish(harness).await;
    }

    let mut harness = setup_with_validity_and_budget(
        GateDecision::Allow(7),
        Duration::from_secs(30),
        8192,
        true,
        0,
    )
    .await;
    harness.stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nTransfer-Encoding: chunked\r\n\r\n").await.expect("write head");
    let mut framed = b"400\r\n".to_vec();
    framed.extend(std::iter::repeat_n(b'x', 1024));
    framed.extend_from_slice(b"\r\n0\r\n\r\n");
    harness
        .stream
        .write_all(&framed)
        .await
        .expect("write chunk");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 503"));
    assert!(body.is_empty());
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
    assert_eq!(harness.budget.in_use(), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn still_invalid_grant_returns_401_without_executor_and_releases_budget() {
    let mut harness =
        setup_with_validity(GateDecision::Allow(7), Duration::from_secs(30), 8192, false).await;
    harness
        .stream
        .write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 1\r\n\r\nx")
        .await
        .expect("write request");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 401"));
    assert_eq!(
        body,
        &[
            0x81, 0xa5, b'e', b'r', b'r', b'o', b'r', 0xac, b'u', b'n', b'a', b'u', b't', b'h',
            b'o', b'r', b'i', b'z', b'e', b'd'
        ]
    );
    assert_eq!(harness.gate.calls.load(Ordering::SeqCst), 1);
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
    assert_eq!(harness.budget.in_use(), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn routing_and_gate_statuses_have_expected_headers_and_bodies() {
    let cases = [
        (GateDecision::Forbidden("denied".to_owned()), b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), "403", None, None),
        (GateDecision::RateLimited { retry_after_secs: 9 }, b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), "429", Some("9"), None),
        (GateDecision::Unavailable { retry_after_secs: 7 }, b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), "503", Some("7"), None),
        (GateDecision::Allow(7), b"GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), "404", None, None),
        (GateDecision::Allow(7), b"POST /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), "405", None, Some("GET")),
        (GateDecision::Allow(7), b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n".as_slice(), "415", None, None),
        (GateDecision::Allow(7), b"GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), "404", None, None),
    ];
    for (decision, request, status, retry_after, allow) in cases {
        let mut harness = setup(decision).await;
        harness
            .stream
            .write_all(request)
            .await
            .expect("write request");
        let (head, body) =
            tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
                .await
                .expect("response");
        assert!(head.starts_with(&format!("HTTP/1.1 {status}")), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("cache-control: no-store"));
        assert!(lower.contains("x-content-type-options: nosniff"));
        assert!(!lower.contains("access-control-"));
        assert!(lower.contains("content-type: application/msgpack"));
        assert!(lower.contains("connection: close"));
        assert_eq!(
            harness.gate.calls.load(Ordering::SeqCst),
            1,
            "gate call count for {status}"
        );
        assert_eq!(
            harness.exec.calls.load(Ordering::SeqCst),
            0,
            "executor must not run for {status}"
        );
        assert!(body.is_empty());
        assert_closed(&mut harness.stream).await;
        assert_eq!(
            head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-type")
                    .then_some(value.trim())
            }),
            Some("application/msgpack")
        );
        assert_eq!(
            head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("retry-after")
                    .then_some(value.trim())
            }),
            retry_after
        );
        assert_eq!(
            head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("allow").then_some(value.trim())
            }),
            allow
        );
        finish(harness).await;
    }
}

#[tokio::test]
async fn non_http11_is_rejected_before_authentication() {
    let mut harness = setup(GateDecision::Allow(7)).await;
    harness
        .stream
        .write_all(b"GET /shamir/v1/ping HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .expect("write HTTP/1.0");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("response");
    assert!(head.starts_with("HTTP/1.1 400"));
    assert!(body.is_empty());
    assert_eq!(harness.gate.calls.load(Ordering::SeqCst), 0);
    assert_closed(&mut harness.stream).await;
    finish(harness).await;
}

#[tokio::test]
async fn client_abort_releases_declared_body_reservation() {
    let harness = setup(GateDecision::Allow(7)).await;
    let Harness {
        mut stream,
        budget,
        shutdown,
        server,
        ..
    } = harness;
    stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\nab").await.expect("partial request");
    tokio::time::timeout(Duration::from_secs(2), async {
        while budget.in_use() != 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("declared reservation");
    drop(stream);
    shutdown.cancel();
    server.await.expect("server task").expect("serve result");
    assert_eq!(budget.in_use(), 0);
}
#[tokio::test]
async fn single_authorization_header_reaches_gate_unchanged() {
    let mut harness = setup(GateDecision::Unauthorized).await;
    harness.stream.write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer exact\r\n\r\n").await.expect("write request");
    let _ = tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
        .await
        .expect("response");
    assert_eq!(
        harness.gate.auth.lock().expect("auth lock").as_slice(),
        &[AuthRecord {
            value: AuthValue::Single(b"Bearer exact".to_vec()),
            forwarded_for: Vec::new()
        }]
    );
    finish(harness).await;
}

#[tokio::test]
async fn authorization_cardinality_is_preserved_at_gate() {
    let mut harness = setup(GateDecision::Unauthorized).await;
    harness.stream.write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: one\r\nAuthorization: two\r\nX-Forwarded-For: 198.51.100.4\r\n\r\n").await.expect("write duplicate headers");
    let _ = tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
        .await
        .expect("response");
    assert_eq!(
        harness.gate.auth.lock().expect("auth lock").as_slice(),
        &[AuthRecord {
            value: AuthValue::Multiple,
            forwarded_for: vec![b"198.51.100.4".to_vec()]
        }]
    );
    finish(harness).await;
}
