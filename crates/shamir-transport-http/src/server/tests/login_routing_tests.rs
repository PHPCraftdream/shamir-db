use crate::gate::{
    AuthHeader, ExecOutcome, ExecStatus, GateDecision, PeerInfo, RestExecutor, TokenGate,
};
use crate::server::routes::{LoginHead, LoginResponse, LoginRoutes, NoLogin};
use crate::{
    config::{EndpointSet, HttpLimits, HttpServerConfig},
    server::service::{handle, Context},
    BodyBudget,
};
use bytes::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::{sync::Arc, time::Duration};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

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

#[tokio::test]
async fn login_path_is_routed_before_bearer_authentication() {
    let (mut client, server) = duplex(8192);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
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
        holds: _tx,
        login: Arc::new(NoLogin),
        write_timeout_override: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    });
    let service = service_fn(move |request| handle(request, context.clone()));
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, rx,
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
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
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
        holds: _tx,
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
