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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
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
    async fn handle(&self, _: Bytes, _: Arc<Self::Permit>, _: Bytes) -> LoginResponse {
        LoginResponse::new(http::StatusCode::OK, Bytes::new())
    }
    fn refused(&self, _: &[u8], _: http::StatusCode) {}
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

struct Counting {
    records: Arc<Mutex<Vec<(String, u16)>>>,
    handle_delay: Option<Duration>,
}

impl LoginRoutes for Counting {
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
    async fn handle(&self, _: Bytes, _: Arc<Self::Permit>, _: Bytes) -> LoginResponse {
        if let Some(delay) = self.handle_delay {
            tokio::time::sleep(delay).await;
        }
        LoginResponse::new(http::StatusCode::OK, Bytes::new())
    }
    fn refused(&self, route: &[u8], status: http::StatusCode) {
        self.records
            .lock()
            .expect("records mutex")
            .push((String::from_utf8_lossy(route).into_owned(), status.as_u16()));
    }
    fn body_limit(&self, _: &[u8]) -> usize {
        4096
    }
}

struct Signal(Arc<AtomicUsize>);
impl Drop for Signal {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct HoldRoutes {
    body: Bytes,
    counter: Arc<AtomicUsize>,
}

impl LoginRoutes for HoldRoutes {
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
    async fn handle(&self, _: Bytes, _: Arc<Self::Permit>, _: Bytes) -> LoginResponse {
        LoginResponse::new(http::StatusCode::OK, self.body.clone())
            .with_hold(Arc::new(Signal(Arc::clone(&self.counter))) as Arc<dyn Send + Sync>)
    }
    fn refused(&self, _: &[u8], _: http::StatusCode) {}
    fn body_limit(&self, _: &[u8]) -> usize {
        4096
    }
}

fn login_context<L: LoginRoutes>(
    login: Arc<L>,
    holds: tokio::sync::mpsc::Sender<Box<dyn Send + Sync>>,
    limits: HttpLimits,
) -> Arc<Context<Gate, Executor, NoWs, L>> {
    let peer = PeerInfo {
        addr: "127.0.0.1:1234".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    Arc::new(Context {
        cfg: Arc::new(HttpServerConfig {
            limits,
            endpoints: EndpointSet {
                rest: true,
                ws: false,
            },
            tls: None,
            hsts: false,
            forbidden_body: Arc::new(|_| Bytes::new()),
            ping_body: Bytes::new(),
        }),
        gate: Arc::new(Gate),
        exec: Arc::new(Executor),
        peer,
        holds,
        ws: Arc::new(NoWs),
        upgrade: tokio::sync::Mutex::new(None),
        login,
        write_timeout_override: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    })
}

async fn serve_login<L: LoginRoutes>(
    login: Arc<L>,
    limits: HttpLimits,
    write_timeout: Duration,
) -> (
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<Result<(), hyper::Error>>,
) {
    let (client, server) = duplex(8192);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    let context = login_context(login, hold_tx, limits);
    let service = service_fn(move |request| handle(request, context.clone()));
    let io = TokioIo::new(
        crate::framing::guarded_io::GuardedIo::new(server, 8192, 64, false, false, hold_rx)
            .with_timeouts(
                Duration::from_secs(5),
                Duration::from_secs(30),
                write_timeout,
            ),
    );
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    (client, task)
}

const BEGIN_REQUEST: &[u8] = b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n";

#[tokio::test]
async fn refusal_before_handle_is_counted_exactly_once() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let counting = Counting {
        records: Arc::clone(&records),
        handle_delay: None,
    };
    let (mut client, task) = serve_login(
        Arc::new(counting),
        HttpLimits::new(4096, Arc::new(BodyBudget::new(8192))),
        Duration::from_secs(120),
    )
    .await;
    client
        .write_all(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(response.starts_with(b"HTTP/1.1 415"));
    assert_eq!(
        records.lock().expect("records mutex").as_slice(),
        [("/shamir/v1/auth/begin".to_string(), 415_u16)]
    );
}

#[tokio::test]
async fn handler_timeout_after_handle_is_not_counted_by_the_transport() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let counting = Counting {
        records: Arc::clone(&records),
        handle_delay: Some(Duration::from_secs(1)),
    };
    let mut limits = HttpLimits::new(4096, Arc::new(BodyBudget::new(8192)));
    limits.request_timeout = Duration::from_millis(100);
    limits.write_timeout = Duration::from_secs(5);
    let (mut client, task) = serve_login(Arc::new(counting), limits, Duration::from_secs(5)).await;
    client.write_all(BEGIN_REQUEST).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(response.starts_with(b"HTTP/1.1 408"));
    assert!(records.lock().expect("records mutex").is_empty());
}

#[tokio::test]
async fn hold_is_released_only_after_the_client_reads_the_whole_response() {
    let counter = Arc::new(AtomicUsize::new(0));
    let routes = HoldRoutes {
        body: Bytes::from(vec![0xAB_u8; 64 * 1024]),
        counter: Arc::clone(&counter),
    };
    let (mut client, task) = serve_login(
        Arc::new(routes),
        HttpLimits::new(8192, Arc::new(BodyBudget::new(8192))),
        Duration::from_secs(120),
    )
    .await;
    client.write_all(BEGIN_REQUEST).await.unwrap();
    let mut head = [0_u8; 64];
    client
        .read_exact(&mut head)
        .await
        .expect("read response head");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut rest))
        .await
        .expect("response deadline")
        .expect("read response");
    assert!(rest.len() + head.len() >= 64 * 1024);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while counter.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "hold was not released after the response was fully read"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn hold_is_released_once_when_a_stalled_client_aborts_the_connection() {
    let counter = Arc::new(AtomicUsize::new(0));
    let routes = HoldRoutes {
        body: Bytes::from(vec![0xCD_u8; 64 * 1024]),
        counter: Arc::clone(&counter),
    };
    let (mut client, task) = serve_login(
        Arc::new(routes),
        HttpLimits::new(8192, Arc::new(BodyBudget::new(8192))),
        Duration::from_millis(300),
    )
    .await;
    client.write_all(BEGIN_REQUEST).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "stalled connection never released the hold"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
}
