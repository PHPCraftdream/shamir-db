use super::super::{serve, serve_with_ws};
use crate::{
    budget::BodyBudget,
    config::{EndpointSet, HttpLimits, HttpServerConfig},
    gate::{
        AuthHeader, ConnAdmission, ExecOutcome, GateDecision, PeerInfo, RestExecutor, TokenGate,
    },
    server::routes::ws::{HeadOk, UpgradeHead, UpgradedIo, WsReject, WsRoute, WsUpgrade},
};
use bytes::Bytes;
use http::{HeaderName, HeaderValue, StatusCode};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

struct Gate;
impl TokenGate for Gate {
    type Grant = ();
    async fn authenticate(
        &self,
        _: AuthHeader<'_>,
        _: crate::config::Endpoint,
        _: PeerInfo,
    ) -> GateDecision<()> {
        GateDecision::Allow(())
    }
    fn still_valid(&self, _: &()) -> bool {
        true
    }
}
struct Executor;
impl RestExecutor<()> for Executor {
    async fn execute(&self, _: &(), _: Bytes) -> ExecOutcome {
        ExecOutcome {
            status: crate::gate::ExecStatus::Ok,
            body: Bytes::new(),
            retry_after_secs: None,
            hold: None,
        }
    }
}
struct Admission(Arc<AtomicUsize>);
impl ConnAdmission for Admission {
    fn admit(&self, _: &SocketAddr) -> Option<Box<dyn Send + Sync>> {
        self.0.fetch_add(1, Ordering::AcqRel);
        Some(Box::new(CountGuard(self.0.clone())))
    }
}
struct CountGuard(Arc<AtomicUsize>);
impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Hook {
    active: Arc<AtomicUsize>,
    check_calls: Arc<AtomicUsize>,
    done: CancellationToken,
    rejected: bool,
    run_cancelled: CancellationToken,
}
impl WsUpgrade<()> for Hook {
    type HeadState = ();
    type Session = ();
    fn check_head(
        &self,
        _: WsRoute,
        _: UpgradeHead<'_>,
        _: PeerInfo,
    ) -> Result<HeadOk<()>, WsReject> {
        self.check_calls.fetch_add(1, Ordering::AcqRel);
        Ok(HeadOk { state: () })
    }
    fn admit(
        &self,
        _: WsRoute,
        _: HeadOk<()>,
        _: Option<()>,
        _: PeerInfo,
    ) -> Result<((), Vec<(HeaderName, HeaderValue)>), WsReject> {
        if self.rejected {
            return Err(WsReject {
                status: StatusCode::SERVICE_UNAVAILABLE,
                retry_after: Some(1),
                close: true,
            });
        }
        Ok((
            (),
            vec![
                (
                    HeaderName::from_static("sec-websocket-extensions"),
                    HeaderValue::from_static("permessage-deflate"),
                ),
                (
                    HeaderName::from_static("sec-websocket-protocol"),
                    HeaderValue::from_static("shamir-v1"),
                ),
            ],
        ))
    }
    async fn run(&self, _: (), mut io: UpgradedIo, shutdown: CancellationToken) {
        self.active.fetch_add(1, Ordering::AcqRel);
        let mut buf = [0_u8; 4];
        tokio::select! {
            _ = shutdown.cancelled() => { self.run_cancelled.cancel(); }
            result = io.read_exact(&mut buf) => { let _ = result; }
        }
        self.active.fetch_sub(1, Ordering::AcqRel);
        self.done.cancel();
    }
}
fn config() -> Arc<HttpServerConfig> {
    Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, Arc::new(BodyBudget::new(1024))),
        endpoints: EndpointSet {
            rest: true,
            ws: true,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    })
}
async fn listener() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local address");
    (listener, addr)
}
async fn read_response(client: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(5), read_response_head(client))
        .await
        .expect("HTTP response head deadline")
        .expect("valid HTTP response head")
}

async fn read_response_head(client: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() >= 8 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP response head exceeds 8 KiB",
            ));
        }
        client.read_exact(&mut byte).await?;
        response.push(byte[0]);
    }
    Ok(response)
}
fn hook(
    active: Arc<AtomicUsize>,
    check_calls: Arc<AtomicUsize>,
    done: CancellationToken,
    rejected: bool,
    run_cancelled: CancellationToken,
) -> Arc<Hook> {
    Arc::new(Hook {
        active,
        check_calls,
        done,
        rejected,
        run_cancelled,
    })
}

#[tokio::test]
async fn ws_route_returns_hook_headers_and_holds_permit_through_session() {
    let (listener, addr) = listener().await;
    let permits = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let done = CancellationToken::new();
    let shutdown = CancellationToken::new();
    let run_cancelled = CancellationToken::new();
    let server = tokio::spawn(serve_with_ws(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(permits.clone())),
        shutdown.clone(),
        hook(
            active.clone(),
            Arc::new(AtomicUsize::new(0)),
            done.clone(),
            false,
            run_cancelled.clone(),
        ),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write request");
    let response = read_response(&mut client).await;
    assert!(response.starts_with(b"HTTP/1.1 101"));
    assert!(response
        .windows(b"sec-websocket-protocol: shamir-v1".len())
        .any(|w| w.eq_ignore_ascii_case(b"sec-websocket-protocol: shamir-v1")));
    assert!(!response
        .windows(b"sec-websocket-extensions".len())
        .any(|w| w.eq_ignore_ascii_case(b"sec-websocket-extensions")));
    assert_eq!(permits.load(Ordering::Acquire), 1);
    client.write_all(b"ping").await.expect("write tunnel data");
    tokio::time::timeout(Duration::from_secs(2), done.cancelled())
        .await
        .expect("session completes");
    drop(client);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server task deadline")
        .expect("server task")
        .expect("server result");
    assert_eq!(permits.load(Ordering::Acquire), 0);
    assert_eq!(active.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn serve_uses_no_ws_for_native_and_browser_routes() {
    let (listener, addr) = listener().await;
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        shutdown.clone(),
    ));
    for path in ["/shamir/v1/ws", "/shamir/v1/ws/browser"] {
        let mut client = TcpStream::connect(addr).await.expect("connect");
        client
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .expect("write request");
        assert!(read_response(&mut client)
            .await
            .starts_with(b"HTTP/1.1 404"));
    }
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server task deadline")
        .expect("server task")
        .expect("server result");
}

#[tokio::test]
async fn non_get_ws_request_is_rejected_before_hook() {
    let (listener, addr) = listener().await;
    let shutdown = CancellationToken::new();
    let check_calls = Arc::new(AtomicUsize::new(0));
    let server = tokio::spawn(serve_with_ws(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        shutdown.clone(),
        hook(
            Arc::new(AtomicUsize::new(0)),
            check_calls.clone(),
            CancellationToken::new(),
            false,
            CancellationToken::new(),
        ),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"POST /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("write request");
    let response = read_response(&mut client).await;
    assert!(response.starts_with(b"HTTP/1.1 405"));
    assert!(response
        .windows(b"allow: GET".len())
        .any(|w| w.eq_ignore_ascii_case(b"allow: GET")));
    assert_eq!(check_calls.load(Ordering::Acquire), 0);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server task deadline")
        .expect("server task")
        .expect("server result");
}

#[tokio::test]
async fn hook_rejection_returns_status_and_retry_after() {
    let (listener, addr) = listener().await;
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(serve_with_ws(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        shutdown.clone(),
        hook(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            CancellationToken::new(),
            true,
            CancellationToken::new(),
        ),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write request");
    let response = read_response(&mut client).await;
    assert!(response.starts_with(b"HTTP/1.1 503"));
    assert!(response
        .windows(b"retry-after: 1".len())
        .any(|w| w.eq_ignore_ascii_case(b"retry-after: 1")));
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server task deadline")
        .expect("server task")
        .expect("server result");
}

#[tokio::test]
async fn shutdown_cancels_run_token_for_upgraded_session() {
    let (listener, addr) = listener().await;
    let shutdown = CancellationToken::new();
    let run_cancelled = CancellationToken::new();
    let server = tokio::spawn(serve_with_ws(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        shutdown.clone(),
        hook(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            CancellationToken::new(),
            false,
            run_cancelled.clone(),
        ),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write request");
    assert!(read_response(&mut client)
        .await
        .starts_with(b"HTTP/1.1 101"));
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), run_cancelled.cancelled())
        .await
        .expect("run received shutdown");
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server task deadline")
        .expect("server task")
        .expect("server result");
}
