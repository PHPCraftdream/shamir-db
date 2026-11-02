use crate::config::{EndpointSet, HttpLimits, HttpServerConfig};
use crate::gate::{
    AuthHeader, ConnAdmission, ExecOutcome, ExecStatus, GateDecision, PeerInfo, RestExecutor,
    TokenGate,
};
use crate::{serve, ServeError};
use bytes::Bytes;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

struct Gate(GateDecision<u8>);
impl TokenGate for Gate {
    type Grant = u8;
    fn authenticate<'a>(
        &'a self,
        _: AuthHeader<'a>,
        _: crate::config::Endpoint,
        _: PeerInfo,
    ) -> impl Future<Output = GateDecision<u8>> + Send + 'a {
        let decision = match &self.0 {
            GateDecision::Allow(value) => GateDecision::Allow(*value),
            GateDecision::Unauthorized => GateDecision::Unauthorized,
            GateDecision::Forbidden(code) => GateDecision::Forbidden(code.clone()),
            GateDecision::RateLimited { retry_after_secs } => GateDecision::RateLimited {
                retry_after_secs: *retry_after_secs,
            },
            GateDecision::Unavailable { retry_after_secs } => GateDecision::Unavailable {
                retry_after_secs: *retry_after_secs,
            },
        };
        async move { decision }
    }
    fn still_valid(&self, _: &u8) -> bool {
        true
    }
}

struct RevalidateUnavailable;
impl TokenGate for RevalidateUnavailable {
    type Grant = u8;
    fn authenticate<'a>(
        &'a self,
        _: AuthHeader<'a>,
        _: crate::config::Endpoint,
        _: PeerInfo,
    ) -> impl Future<Output = GateDecision<u8>> + Send + 'a {
        async { GateDecision::Allow(1) }
    }
    fn still_valid(&self, _: &u8) -> bool {
        true
    }
    fn revalidate(&self, _: &u8) -> GateDecision<()> {
        GateDecision::Unavailable {
            retry_after_secs: 3,
        }
    }
}
struct RevalidateForbidden;
impl TokenGate for RevalidateForbidden {
    type Grant = u8;
    fn authenticate<'a>(
        &'a self,
        _: AuthHeader<'a>,
        _: crate::config::Endpoint,
        _: PeerInfo,
    ) -> impl Future<Output = GateDecision<u8>> + Send + 'a {
        async { GateDecision::Allow(1) }
    }
    fn still_valid(&self, _: &u8) -> bool {
        true
    }
    fn revalidate(&self, _: &u8) -> GateDecision<()> {
        GateDecision::Forbidden("token_database_denied".into())
    }
}
struct Executor {
    status: ExecStatus,
    retry: Option<u64>,
    calls: AtomicUsize,
}
impl RestExecutor<u8> for Executor {
    fn execute<'a>(&'a self, _: &'a u8, _: Bytes) -> impl Future<Output = ExecOutcome> + Send + 'a {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let status = self.status;
        let retry = self.retry;
        async move {
            ExecOutcome {
                status,
                body: Bytes::from_static(b"wire-body"),
                retry_after_secs: retry,
                hold: None,
            }
        }
    }
}
struct Admission;
impl ConnAdmission for Admission {
    fn admit(&self, _: &SocketAddr) -> Option<Box<dyn Send + Sync>> {
        Some(Box::new(()))
    }
}
struct Harness {
    stream: TcpStream,
    executor: Arc<Executor>,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), ServeError>>,
}
async fn setup(gate: GateDecision<u8>, status: ExecStatus, retry: Option<u64>) -> Harness {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let executor = Arc::new(Executor {
        status,
        retry,
        calls: AtomicUsize::new(0),
    });
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, Arc::new(crate::BodyBudget::new(4096))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    });
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        cfg,
        Arc::new(Gate(gate)),
        executor.clone(),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr))
        .await
        .expect("connect deadline")
        .expect("connect");
    Harness {
        stream,
        executor,
        shutdown,
        server,
    }
}
async fn request(harness: &mut Harness, head: &str) -> (String, Vec<u8>) {
    harness
        .stream
        .write_all(head.as_bytes())
        .await
        .expect("write request");
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(
                harness
                    .stream
                    .read_u8()
                    .await
                    .expect("read response headers"),
            );
        }
        let text = String::from_utf8(head).expect("headers are ASCII");
        let len = text
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0);
        let mut body = vec![0; len];
        harness
            .stream
            .read_exact(&mut body)
            .await
            .expect("read response body");
        (text, body)
    })
    .await
    .expect("response deadline")
}
async fn finish(harness: Harness) {
    harness.shutdown.cancel();
    harness
        .server
        .await
        .expect("server join")
        .expect("serve shutdown");
}

#[tokio::test]
async fn executor_rate_limit_sends_retry_after_seven_on_wire() {
    let mut harness = setup(GateDecision::Allow(1), ExecStatus::RateLimited, Some(7)).await;
    let (head, _) = request(&mut harness, "POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 429"));
    assert!(head.to_ascii_lowercase().contains("retry-after: 7\r\n"));
    finish(harness).await;
}

#[tokio::test]
async fn executor_unavailable_sends_close_and_default_retry_after_one() {
    let mut harness = setup(GateDecision::Allow(1), ExecStatus::Unavailable, None).await;
    let (head, _) = request(&mut harness, "POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n").await;
    let lower = head.to_ascii_lowercase();
    assert!(head.starts_with("HTTP/1.1 503"));
    assert!(lower.contains("connection: close\r\n"));
    assert!(lower.contains("retry-after: 1\r\n"));
    let mut byte = [0; 1];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), harness.stream.read(&mut byte))
            .await
            .expect("EOF deadline")
            .expect("read EOF"),
        0
    );
    finish(harness).await;
}

#[tokio::test]
async fn unauthorized_is_fixed_body_with_bearer_challenge() {
    let mut harness = setup(GateDecision::Unauthorized, ExecStatus::Ok, None).await;
    let (head, body) = request(
        &mut harness,
        "GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 401"));
    assert!(head
        .to_ascii_lowercase()
        .contains("www-authenticate: bearer\r\n"));
    assert_eq!(body, b"\x81\xa5error\xacunauthorized");
    assert_eq!(harness.executor.calls.load(Ordering::SeqCst), 0);
    finish(harness).await;
}

#[tokio::test]
async fn post_body_revalidation_unavailable_closes_without_executor() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let executor = Arc::new(Executor {
        status: ExecStatus::Ok,
        retry: None,
        calls: AtomicUsize::new(0),
    });
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, Arc::new(crate::BodyBudget::new(4096))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    });
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        cfg,
        Arc::new(RevalidateUnavailable),
        executor.clone(),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let mut stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr))
        .await
        .expect("connect deadline")
        .expect("connect");
    stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 3\r\n\r\nabc").await.expect("write request");
    let mut head = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.expect("read headers"));
        }
    })
    .await
    .expect("response deadline");
    let head = String::from_utf8(head)
        .expect("headers are ASCII")
        .to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 503"));
    assert!(head.contains("connection: close\r\n"));
    assert!(head.contains("retry-after: 3\r\n"));
    let mut body = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut body))
        .await
        .expect("body deadline")
        .expect("read response body");
    assert!(body.is_empty());
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    shutdown.cancel();
    server.await.expect("server join").expect("serve shutdown");
}

#[tokio::test]
async fn post_body_revalidation_forbidden_preserves_configured_code_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("listener address");
    let executor = Arc::new(Executor {
        status: ExecStatus::Ok,
        retry: None,
        calls: AtomicUsize::new(0),
    });
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, Arc::new(crate::BodyBudget::new(4096))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|code| Bytes::copy_from_slice(code.as_bytes())),
        ping_body: Bytes::new(),
    });
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        cfg,
        Arc::new(RevalidateForbidden),
        executor.clone(),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let mut stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr))
        .await
        .expect("connect deadline")
        .expect("connect");
    stream.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 3\r\n\r\nabc").await.expect("write request");
    let mut head = Vec::new();
    let body_len = tokio::time::timeout(Duration::from_secs(2), async {
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.expect("read headers"));
        }
        String::from_utf8_lossy(&head)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0)
    })
    .await
    .expect("response deadline");
    let head = String::from_utf8(head)
        .expect("headers are ASCII")
        .to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 403"));
    assert!(head.contains("connection: close\r\n"));
    assert!(!head.contains("retry-after:"));
    let mut body = vec![0; body_len];
    tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut body))
        .await
        .expect("body deadline")
        .expect("read response body");
    assert_eq!(body, b"token_database_denied");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    shutdown.cancel();
    server.await.expect("server join").expect("serve shutdown");
}
