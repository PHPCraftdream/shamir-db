use crate::gate::{
    AuthHeader, ConnAdmission, ExecOutcome, ExecStatus, GateDecision, PeerInfo, RestExecutor,
    TokenGate,
};
use crate::server::accept::NoWs;
use crate::server::routes::ws::{
    HeadOk, UpgradeHead, UpgradedIo, WsAdmission, WsReject, WsRoute, WsUpgrade,
};
use crate::server::routes::{LoginHead, LoginResponse, LoginRoutes, NoLogin, RouteServices};
use crate::{
    config::{EndpointSet, HttpLimits, HttpServerConfig},
    server::{
        accept::serve_with_routes,
        service::{handle, Context},
    },
    BodyBudget,
};
use bytes::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{duplex, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Gate;
impl TokenGate for Gate {
    type Grant = ();
    async fn authenticate(
        &self,
        _: AuthHeader<'_>,
        _: crate::config::Endpoint,
        peer: PeerInfo,
    ) -> GateDecision<Self::Grant> {
        assert!(peer.addr.port() > 0);
        GateDecision::Unauthorized
    }
    fn still_valid(&self, _: &Self::Grant) -> bool {
        true
    }
}

struct Executor;
impl RestExecutor<()> for Executor {
    async fn execute(&self, _: &(), _: Bytes) -> ExecOutcome {
        ExecOutcome {
            status: ExecStatus::Ok,
            body: Bytes::new(),
            hold: None,
            retry_after_secs: None,
        }
    }
}

struct Enabled;
impl LoginRoutes for Enabled {
    type Permit = ();
    fn enabled(&self) -> bool {
        true
    }
    async fn admit_head(
        &self,
        _: &[u8],
        _: LoginHead<'_>,
        _: PeerInfo,
    ) -> Result<Self::Permit, LoginResponse> {
        Ok(())
    }
    async fn handle(&self, _: Bytes, _: Self::Permit, _: Bytes) -> LoginResponse {
        LoginResponse::new(http::StatusCode::OK, Bytes::new())
    }
    fn body_limit(&self, _: &[u8]) -> usize {
        4096
    }
}

struct RejectingWs;
impl WsUpgrade<()> for RejectingWs {
    type HeadState = ();
    type Session = ();

    fn check_head(
        &self,
        _route: WsRoute,
        _head: UpgradeHead<'_>,
        _peer: PeerInfo,
    ) -> Result<HeadOk<Self::HeadState>, WsReject> {
        Err(WsReject {
            status: http::StatusCode::UPGRADE_REQUIRED,
            retry_after: None,
            close: true,
        })
    }

    fn admit(
        &self,
        _route: WsRoute,
        _head: HeadOk<Self::HeadState>,
        _grant: Option<()>,
        _peer: PeerInfo,
    ) -> Result<WsAdmission<Self::Session>, WsReject> {
        unreachable!("head rejection prevents admission")
    }

    async fn run(
        &self,
        _session: Self::Session,
        _io: UpgradedIo,
        _shutdown: tokio_util::sync::CancellationToken,
    ) {
    }
}

struct Admission;
impl ConnAdmission for Admission {
    fn admit(&self, _peer: &std::net::SocketAddr) -> Option<Box<dyn Send + Sync>> {
        Some(Box::new(()))
    }
}

async fn request_status(addr: std::net::SocketAddr, request: &'static [u8]) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect listener");
    stream.write_all(request).await.expect("write request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .expect("response deadline")
        .expect("read response");
    response
}

#[tokio::test]
async fn login_path_is_routed_before_bearer_authentication() {
    let (mut client, server) = duplex(8192);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    let peer = PeerInfo {
        addr: "127.0.0.1:1234".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(4096, Arc::new(BodyBudget::new(8192))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    });
    let context = Arc::new(Context {
        cfg,
        gate: Arc::new(Gate),
        exec: Arc::new(Executor),
        peer,
        holds: hold_tx,
        ws: Arc::new(NoWs),
        upgrade: tokio::sync::Mutex::new(None),
        login: Arc::new(NoLogin),
        write_timeout_override: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    });
    let service = service_fn(move |request| handle(request, context.clone()));
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, hold_rx,
    ));
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    client
        .write_all(b"GET /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(response.starts_with(b"HTTP/1.1 404"));
    assert!(!response.starts_with(b"HTTP/1.1 401"));
}

#[tokio::test]
async fn enabled_login_does_not_change_bearer_gate_response() {
    let (mut client, server) = duplex(8192);
    let (holds, rx) = tokio::sync::mpsc::channel(1);
    let peer = PeerInfo {
        addr: "127.0.0.1:1234".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(4096, Arc::new(BodyBudget::new(8192))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    });
    let context = Arc::new(Context {
        cfg,
        gate: Arc::new(Gate),
        exec: Arc::new(Executor),
        peer,
        holds,
        ws: Arc::new(NoWs),
        upgrade: tokio::sync::Mutex::new(None),
        login: Arc::new(Enabled),
        write_timeout_override: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    });
    let service = service_fn(move |request| handle(request, context.clone()));
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, rx,
    ));
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    client
        .write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(response.starts_with(b"HTTP/1.1 401"));
    assert!(response
        .windows(b"www-authenticate: Bearer".len())
        .any(|w| w.eq_ignore_ascii_case(b"www-authenticate: Bearer")));
    assert!(!response
        .windows(b"pragma:".len())
        .any(|w| w.eq_ignore_ascii_case(b"pragma:")));
}

#[tokio::test]
async fn enabled_login_with_ws_hook_precedes_bearer_gate() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind HTTP listener");
    let address = listener.local_addr().expect("listener address");
    let config = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(4096, Arc::new(BodyBudget::new(8192))),
        endpoints: EndpointSet {
            rest: true,
            ws: true,
        },
        tls: None,
        hsts: false,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    });
    let shutdown = tokio_util::sync::CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(serve_with_routes(
        listener,
        config,
        Arc::new(Gate),
        Arc::new(Executor),
        Arc::new(Admission),
        server_shutdown,
        RouteServices {
            login: Arc::new(Enabled),
            ws: Arc::new(RejectingWs),
        },
    ));

    let login = request_status(
        address,
        b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(login.starts_with(b"HTTP/1.1 200"));

    for path in ["/shamir/v1/ws", "/shamir/v1/ws/browser"] {
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let response = request_status_owned(address, request).await;
        assert!(response.starts_with(b"HTTP/1.1 426"));
    }

    let rest = request_status(
        address,
        b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(rest.starts_with(b"HTTP/1.1 401"));
    assert!(rest
        .windows(b"www-authenticate: Bearer".len())
        .any(|window| window.eq_ignore_ascii_case(b"www-authenticate: Bearer")));

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown deadline")
        .expect("server task join")
        .expect("server serve result");
}

async fn request_status_owned(addr: std::net::SocketAddr, request: String) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect listener");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .expect("response deadline")
        .expect("read response");
    response
}
