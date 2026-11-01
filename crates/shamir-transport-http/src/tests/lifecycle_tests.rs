use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{oneshot, Notify},
};
use tokio_util::sync::CancellationToken;

use crate::{
    serve, AuthHeader, BodyBudget, Endpoint, EndpointSet, ExecOutcome, ExecStatus, GateDecision,
    HttpLimits, HttpServerConfig, PeerInfo, RestExecutor, ServeError, TokenGate,
};

struct LifecycleGate;
impl TokenGate for LifecycleGate {
    type Grant = ();

    async fn authenticate(
        &self,
        _: AuthHeader<'_>,
        _: Endpoint,
        _: PeerInfo,
    ) -> GateDecision<Self::Grant> {
        GateDecision::Allow(())
    }

    fn still_valid(&self, _: &Self::Grant) -> bool {
        true
    }
}

struct BlockingExecutor {
    entered: Arc<Notify>,
    release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}
impl RestExecutor<()> for BlockingExecutor {
    async fn execute(&self, _: &(), _: Bytes) -> ExecOutcome {
        self.entered.notify_one();
        if let Some(release) = self.release.lock().await.take() {
            let _ = release.await;
        }
        ExecOutcome {
            status: ExecStatus::Ok,
            body: Bytes::new(),
            hold: None,
        }
    }
}

struct Admission;
impl crate::ConnAdmission for Admission {
    fn admit(&self, _: &std::net::SocketAddr) -> Option<Box<dyn Send + Sync>> {
        Some(Box::new(()))
    }
}

fn config() -> Arc<HttpServerConfig> {
    Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, Arc::new(BodyBudget::new(4096))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    })
}

#[tokio::test]
async fn cancellation_waits_for_active_executor_then_finishes_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let entered = Arc::new(Notify::new());
    let (release_tx, release_rx) = oneshot::channel();
    let executor = Arc::new(BlockingExecutor {
        entered: entered.clone(),
        release: tokio::sync::Mutex::new(Some(release_rx)),
    });
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(serve(
        listener,
        config(),
        Arc::new(LifecycleGate),
        executor,
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n").await.expect("request");
    entered.notified().await;
    shutdown.cancel();
    assert!(
        !task.is_finished(),
        "active handler must remain alive during grace"
    );
    release_tx.send(()).expect("release executor");
    task.await.expect("serve task").expect("serve result");
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .expect("connection closes");
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200"));
    let (head, body) = response.split_once("\r\n\r\n").expect("response head");
    let content_length = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .expect("content length");
    assert_eq!(body.len(), content_length);
}

struct ObservedAdmission {
    accepted: Arc<Notify>,
}
impl crate::ConnAdmission for ObservedAdmission {
    fn admit(&self, _: &std::net::SocketAddr) -> Option<Box<dyn Send + Sync>> {
        self.accepted.notify_one();
        Some(Box::new(()))
    }
}

struct CountingGate {
    calls: Arc<AtomicUsize>,
}
impl TokenGate for CountingGate {
    type Grant = ();

    fn authenticate<'a>(
        &'a self,
        _: AuthHeader<'a>,
        _: Endpoint,
        _: PeerInfo,
    ) -> impl Future<Output = GateDecision<Self::Grant>> + Send + 'a {
        self.calls.fetch_add(1, Ordering::SeqCst);
        async { GateDecision::Allow(()) }
    }

    fn still_valid(&self, _: &Self::Grant) -> bool {
        true
    }
}

struct CountingExecutor {
    calls: Arc<AtomicUsize>,
}
impl RestExecutor<()> for CountingExecutor {
    fn execute<'a>(&'a self, _: &'a (), _: Bytes) -> impl Future<Output = ExecOutcome> + Send + 'a {
        self.calls.fetch_add(1, Ordering::SeqCst);
        async {
            ExecOutcome {
                status: ExecStatus::Ok,
                body: Bytes::new(),
                hold: None,
            }
        }
    }
}

fn observed_config(body_budget: Arc<BodyBudget>) -> Arc<HttpServerConfig> {
    let mut cfg = (*config()).clone();
    cfg.limits.body_budget = body_budget;
    cfg.limits.head_timeout = Duration::from_secs(1);
    cfg.limits.body_timeout = Duration::from_secs(2);
    cfg.limits.request_timeout = Duration::from_secs(10);
    Arc::new(cfg)
}

async fn read_timeout_response(client: &mut TcpStream) -> Vec<u8> {
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .expect("server closes timed-out connection");
    response
}

#[tokio::test(start_paused = true)]
async fn slow_request_head_times_out_with_408_and_closes_without_authentication() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let admitted = Arc::new(Notify::new());
    let gate_calls = Arc::new(AtomicUsize::new(0));
    let executor_calls = Arc::new(AtomicUsize::new(0));
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        observed_config(Arc::new(BodyBudget::new(4))),
        Arc::new(CountingGate {
            calls: gate_calls.clone(),
        }),
        Arc::new(CountingExecutor {
            calls: executor_calls.clone(),
        }),
        Arc::new(ObservedAdmission {
            accepted: admitted.clone(),
        }),
        shutdown.clone(),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost:")
        .await
        .expect("partial request head");
    admitted.notified().await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let response = read_timeout_response(&mut client).await;
    assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout\r\n"));
    assert!(response
        .windows(17)
        .any(|part| part == b"Connection: close"));
    assert_eq!(gate_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor_calls.load(Ordering::SeqCst), 0);
    shutdown.cancel();
    server.await.expect("serve task").expect("serve result");
}

#[tokio::test(start_paused = true)]
async fn incomplete_request_body_times_out_with_408_and_releases_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let admitted = Arc::new(Notify::new());
    let gate_calls = Arc::new(AtomicUsize::new(0));
    let executor_calls = Arc::new(AtomicUsize::new(0));
    let budget = Arc::new(BodyBudget::new(4));
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        observed_config(budget.clone()),
        Arc::new(CountingGate {
            calls: gate_calls.clone(),
        }),
        Arc::new(CountingExecutor {
            calls: executor_calls.clone(),
        }),
        Arc::new(ObservedAdmission {
            accepted: admitted.clone(),
        }),
        shutdown.clone(),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\nab")
        .await
        .expect("partial request body");
    admitted.notified().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while budget.in_use() != 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("declared body budget reservation");
    assert_eq!(gate_calls.load(Ordering::SeqCst), 1);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    let response = read_timeout_response(&mut client).await;
    assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout\r\n"));
    assert!(response
        .windows(17)
        .any(|part| part == b"Connection: close"));
    assert_eq!(budget.in_use(), 0);
    assert_eq!(executor_calls.load(Ordering::SeqCst), 0);
    shutdown.cancel();
    server.await.expect("serve task").expect("serve result");
}

#[tokio::test]
async fn allowed_expect_continue_body_stall_times_out_after_interim_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let admitted = Arc::new(Notify::new());
    let gate_calls = Arc::new(AtomicUsize::new(0));
    let executor_calls = Arc::new(AtomicUsize::new(0));
    let budget = Arc::new(BodyBudget::new(4));
    let mut cfg = (*observed_config(budget.clone())).clone();
    cfg.limits.head_timeout = Duration::from_secs(5);
    cfg.limits.body_timeout = Duration::from_secs(2);
    cfg.limits.request_timeout = Duration::from_secs(10);
    cfg.limits.write_timeout = Duration::from_secs(5);
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        Arc::new(cfg),
        Arc::new(CountingGate {
            calls: gate_calls.clone(),
        }),
        Arc::new(CountingExecutor {
            calls: executor_calls.clone(),
        }),
        Arc::new(ObservedAdmission {
            accepted: admitted.clone(),
        }),
        shutdown.clone(),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n")
        .await
        .expect("request head");
    admitted.notified().await;
    let mut interim = Vec::new();
    let mut byte = [0; 1];
    tokio::time::timeout(Duration::from_secs(4), async {
        while !interim.ends_with(b"\r\n\r\n") {
            client
                .read_exact(&mut byte)
                .await
                .expect("interim response");
            interim.push(byte[0]);
        }
    })
    .await
    .expect("flushed interim response");
    assert_eq!(interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"ab").await.expect("partial body");
    tokio::time::timeout(Duration::from_secs(4), async {
        while budget.in_use() != 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("declared body budget reservation");
    assert_eq!(gate_calls.load(Ordering::SeqCst), 1);
    tokio::task::yield_now().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    let response = tokio::time::timeout(Duration::from_secs(3), read_timeout_response(&mut client))
        .await
        .expect("bounded final response read");
    assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout\r\n"));
    assert!(response
        .windows(17)
        .any(|part| part == b"Connection: close"));
    let (head, body) = String::from_utf8_lossy(&response)
        .split_once("\r\n\r\n")
        .map(|(head, body)| (head.to_owned(), body.to_owned()))
        .expect("final response head");
    assert!(head
        .lines()
        .any(|line| line.eq_ignore_ascii_case("content-length: 0")));
    assert!(head
        .lines()
        .any(|line| line.eq_ignore_ascii_case("cache-control: no-store")));
    assert!(head
        .lines()
        .any(|line| line.eq_ignore_ascii_case("x-content-type-options: nosniff")));
    assert!(!head.to_ascii_lowercase().contains("access-control-"));
    assert!(body.is_empty());
    assert_eq!(
        response
            .windows(25)
            .filter(|part| *part == b"HTTP/1.1 100 Continue\r\n\r\n")
            .count(),
        0
    );
    assert_eq!(budget.in_use(), 0);
    assert_eq!(executor_calls.load(Ordering::SeqCst), 0);
    shutdown.cancel();
    server.await.expect("serve task").expect("serve result");
}

#[tokio::test(start_paused = true)]
async fn idle_keep_alive_connection_ends_with_shutdown_grace() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let mut cfg = (*config()).clone();
    cfg.limits.request_timeout = Duration::from_secs(2);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(serve(
        listener,
        Arc::new(cfg),
        Arc::new(LifecycleGate),
        Arc::new(BlockingExecutor {
            entered: Arc::new(Notify::new()),
            release: tokio::sync::Mutex::new(None),
        }),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("request");
    let mut response_head = Vec::new();
    let mut byte = [0; 1];
    while !response_head.ends_with(b"\r\n\r\n") {
        client.read_exact(&mut byte).await.expect("response head");
        response_head.push(byte[0]);
    }
    let head = String::from_utf8(response_head).expect("HTTP response head");
    assert!(head.starts_with("HTTP/1.1 200"));
    let content_length = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .expect("content length");
    let mut body = vec![0; content_length];
    client
        .read_exact(&mut body)
        .await
        .expect("complete response body");
    shutdown.cancel();
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("serve task finishes within bounded grace")
        .expect("serve task")
        .expect("serve result");
}

#[derive(Debug)]
struct NoCertificate;
impl rustls::server::ResolvesServerCert for NoCertificate {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }
}

#[tokio::test]
async fn tls12_only_configuration_is_rejected_before_accept_loop() {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let tls12_suites = provider
        .cipher_suites
        .into_iter()
        .filter(|suite| suite.version() == &rustls::version::TLS12)
        .collect();
    let provider = rustls::crypto::CryptoProvider {
        cipher_suites: tls12_suites,
        ..rustls::crypto::aws_lc_rs::default_provider()
    };
    let tls_config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .expect("TLS 1.2 provider config")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(NoCertificate));
    let mut cfg = (*config()).clone();
    cfg.tls = Some(Arc::new(tls_config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let error = serve(
        listener,
        Arc::new(cfg),
        Arc::new(LifecycleGate),
        Arc::new(BlockingExecutor {
            entered: Arc::new(Notify::new()),
            release: tokio::sync::Mutex::new(None),
        }),
        Arc::new(Admission),
        CancellationToken::new(),
    )
    .await
    .expect_err("TLS 1.2-only input must fail closed");
    assert!(matches!(error, ServeError::TlsConfiguration(_)));
}
