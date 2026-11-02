use super::accept::{serve, serve_with_ws};
use crate::{
    budget::BodyBudget,
    config::{EndpointSet, HttpLimits, HttpServerConfig},
    gate::{
        AuthHeader, ConnAdmission, ExecOutcome, GateDecision, PeerInfo, RestExecutor, TokenGate,
    },
    server::routes::ws::{HeadOk, UpgradeHead, UpgradedIo, WsReject, WsRoute, WsUpgrade},
};
use bytes::Bytes;
use http::{HeaderName, HeaderValue};
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
    fn authenticate<'a>(
        &'a self,
        _: AuthHeader<'a>,
        _: crate::config::Endpoint,
        _: PeerInfo,
    ) -> impl std::future::Future<Output = GateDecision<()>> + Send + 'a {
        async { GateDecision::Allow(()) }
    }
    fn still_valid(&self, _: &()) -> bool {
        true
    }
}
struct Executor;
impl RestExecutor<()> for Executor {
    fn execute<'a>(
        &'a self,
        _: &'a (),
        _: Bytes,
    ) -> impl std::future::Future<Output = ExecOutcome> + Send + 'a {
        async {
            ExecOutcome {
                status: crate::gate::ExecStatus::Ok,
                body: Bytes::new(),
                retry_after_secs: None,
                hold: None,
            }
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
    done: CancellationToken,
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
        Ok(HeadOk { state: () })
    }
    fn admit(
        &self,
        _: WsRoute,
        _: HeadOk<()>,
        _: Option<()>,
        _: PeerInfo,
    ) -> Result<((), Vec<(HeaderName, HeaderValue)>), WsReject> {
        Ok((
            (),
            vec![(
                HeaderName::from_static("sec-websocket-protocol"),
                HeaderValue::from_static("shamir-v1"),
            )],
        ))
    }
    async fn run(&self, _: (), mut io: UpgradedIo, shutdown: CancellationToken) {
        let active = self.active.clone();
        let done = self.done.clone();
        active.fetch_add(1, Ordering::AcqRel);
        let mut buf = [0_u8; 4];
        tokio::select! {
            _ = shutdown.cancelled() => { let _ = io.write_all(b"stop").await; }
            result = io.read_exact(&mut buf) => { let _ = result; }
        }
        active.fetch_sub(1, Ordering::AcqRel);
        done.cancel();
    }
}
fn config() -> Arc<HttpServerConfig> {
    let budget = Arc::new(BodyBudget::new(1024));
    Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, budget),
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
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener address");
    (listener, addr)
}
#[tokio::test]
async fn ws_route_returns_hook_headers_and_holds_permit_through_session() {
    let (listener, addr) = listener().await;
    let permit_count = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let done = CancellationToken::new();
    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(serve_with_ws(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(permit_count.clone())),
        server_shutdown,
        Arc::new(Hook {
            active: active.clone(),
            done: done.clone(),
        }),
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client.write_all(b"GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await.expect("write request");
    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        client.read_exact(&mut byte).await.expect("read response");
        response.push(byte[0]);
    }
    assert!(response.starts_with(b"HTTP/1.1 101"));
    assert!(response
        .windows(b"sec-websocket-protocol: shamir-v1".len())
        .any(|w| w.eq_ignore_ascii_case(b"sec-websocket-protocol: shamir-v1")));
    assert!(!response
        .windows(b"sec-websocket-extensions".len())
        .any(|w| w.eq_ignore_ascii_case(b"sec-websocket-extensions")));
    assert_eq!(permit_count.load(Ordering::Acquire), 1);
    client.write_all(b"ping").await.expect("write tunnel data");
    tokio::time::timeout(Duration::from_secs(2), done.cancelled())
        .await
        .expect("session completion");
    drop(client);
    shutdown.cancel();
    server.await.expect("server task").expect("server result");
    assert_eq!(permit_count.load(Ordering::Acquire), 0);
    assert_eq!(active.load(Ordering::Acquire), 0);
}
#[tokio::test]
async fn serve_uses_no_ws_and_returns_not_found() {
    let (listener, addr) = listener().await;
    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(serve(
        listener,
        config(),
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission(Arc::new(AtomicUsize::new(0)))),
        server_shutdown,
    ));
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"GET /shamir/v1/ws HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("write request");
    let mut response = [0_u8; 128];
    let n = client.read(&mut response).await.expect("read response");
    assert!(response[..n].starts_with(b"HTTP/1.1 404"));
    shutdown.cancel();
    server.await.expect("server task").expect("server result");
}
