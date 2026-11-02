//! REST authentication redaction and real-wire unauthorized uniformity regressions.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::token::{OwnerFloors, TokenChannels, TokenMode};
use shamir_connect::server::TokenSessionStore;
use shamir_transport_http::{
    serve, AuthHeader, BodyBudget, ConnAdmission, Endpoint, EndpointSet, ExecOutcome, ExecStatus,
    GateDecision, HttpLimits, HttpServerConfig, PeerInfo, RestExecutor, ServeError, TokenGate,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::super::gate::{RestGate, RestGateRails};
use crate::config::TokensConfig;
use crate::security::tokens::{NewToken, TokenService, TokenServiceOptions};

const NOW: u64 = 20_000_000_000;
const GOLDEN_401_BODY: &[u8] = &[
    0x81, 0xa5, b'e', b'r', b'r', b'o', b'r', 0xac, b'u', b'n', b'a', b'u', b't', b'h', b'o', b'r',
    b'i', b'z', b'e', b'd',
];

struct Owners {
    floor: AtomicU64,
    present: AtomicBool,
}
impl OwnerFloors for Owners {
    fn floor_if_present(&self, id: &[u8; 16]) -> Option<u64> {
        (*id == [7; 16] && self.present.load(Ordering::SeqCst))
            .then(|| self.floor.load(Ordering::SeqCst))
    }
}
struct Audit;
impl AuditAppender for Audit {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

struct Fixture {
    service: Arc<TokenService>,
    owners: Arc<Owners>,
    clock: Arc<AtomicU64>,
    gate: Arc<RestGate>,
    _temp: TempDir,
}
async fn fixture() -> Fixture {
    let temp = TempDir::new().expect("tempdir");
    let owners = Arc::new(Owners {
        floor: AtomicU64::new(0),
        present: AtomicBool::new(true),
    });
    let service = Arc::new(
        TokenService::open(TokenServiceOptions {
            data_dir: temp.path().to_path_buf(),
            token_key: Zeroizing::new([3; 32]),
            owners: owners.clone(),
            sessions: Arc::new(TokenSessionStore::new()),
            audit: Arc::new(AuditChainWriter::new(
                AuditChain::new([2; 32]),
                Arc::new(Audit),
            )),
            config: TokensConfig::default(),
            read_only_node: false,
            clock: Arc::new(|| Some(NOW)),
        })
        .await
        .expect("token service"),
    );
    let clock = Arc::new(AtomicU64::new(NOW));
    let gate_clock = clock.clone();
    let gate = Arc::new(RestGate::new(
        service.clone(),
        Arc::new(move || Some(gate_clock.load(Ordering::SeqCst))),
        RestGateRails {
            plaintext_remote: false,
            allow_plaintext_write: false,
        },
    ));
    Fixture {
        service,
        owners,
        clock,
        gate,
        _temp: temp,
    }
}
async fn token(f: &Fixture, label: &str) -> String {
    f.service
        .create(NewToken {
            owner_user_id: [7; 16],
            owner_roles: vec!["reader".into()],
            label: label.into(),
            ttl_secs: Some(60),
            mode: TokenMode::ReadOnly,
            channels: TokenChannels::Rest,
            databases: None,
        })
        .await
        .expect("create token")
        .token
        .as_str()
        .to_owned()
}

#[derive(Clone, Copy)]
enum CredentialCase {
    Missing,
    Multiple,
    Malformed,
    UnknownId,
    WrongSecret,
    Revoked,
    Expired,
    OwnerFloor,
}
fn credential_name(case: CredentialCase) -> &'static str {
    match case {
        CredentialCase::Missing => "missing",
        CredentialCase::Multiple => "multiple",
        CredentialCase::Malformed => "malformed",
        CredentialCase::UnknownId => "unknown-id",
        CredentialCase::WrongSecret => "wrong-secret",
        CredentialCase::Revoked => "revoked",
        CredentialCase::Expired => "expired",
        CredentialCase::OwnerFloor => "owner-floor",
    }
}
async fn authorization(f: &Fixture, case: CredentialCase) -> Option<Vec<u8>> {
    let value = match case {
        CredentialCase::Missing => return None,
        CredentialCase::Multiple => {
            return Some(b"Authorization: Bearer a\r\nAuthorization: Bearer b".to_vec())
        }
        CredentialCase::Malformed => b"malformed".to_vec(),
        CredentialCase::UnknownId => {
            b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_vec()
        }
        _ => {
            let value = token(f, credential_name(case)).await;
            match case {
                CredentialCase::WrongSecret => {
                    let mut bytes = value.into_bytes();
                    bytes[28] = if bytes[28] == b'a' { b'b' } else { b'a' };
                    bytes
                }
                CredentialCase::Revoked => {
                    let parsed = shamir_connect::server::token::parse_token(value.as_bytes())
                        .expect("generated token parses");
                    f.service
                        .revoke(parsed.id, NOW + 1)
                        .await
                        .expect("revoke token");
                    value.into_bytes()
                }
                CredentialCase::Expired => {
                    f.clock.store(NOW + 61_000_000_000, Ordering::SeqCst);
                    value.into_bytes()
                }
                CredentialCase::OwnerFloor => {
                    f.owners.floor.store(NOW, Ordering::SeqCst);
                    value.into_bytes()
                }
                _ => unreachable!(),
            }
        }
    };
    Some([b"Bearer ".as_slice(), &value].concat())
}

struct NeverExec {
    calls: AtomicUsize,
}
impl RestExecutor<shamir_connect::server::token::Verified> for NeverExec {
    fn execute<'a>(
        &'a self,
        _: &'a shamir_connect::server::token::Verified,
        _: Bytes,
    ) -> impl Future<Output = ExecOutcome> + Send + 'a {
        self.calls.fetch_add(1, Ordering::SeqCst);
        async {
            ExecOutcome {
                status: ExecStatus::Ok,
                body: Bytes::from_static(b"unexpected"),
                retry_after_secs: None,
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
struct WireHarness {
    stream: TcpStream,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), ServeError>>,
    exec: Arc<NeverExec>,
}
async fn wire(f: &Fixture) -> WireHarness {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let budget = Arc::new(BodyBudget::new(4096));
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, budget),
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
    let exec = Arc::new(NeverExec {
        calls: AtomicUsize::new(0),
    });
    let server = tokio::spawn(serve(
        listener,
        cfg,
        f.gate.clone(),
        exec.clone(),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    let stream = TcpStream::connect(addr).await.expect("connect");
    WireHarness {
        stream,
        shutdown,
        server,
        exec,
    }
}
async fn assert_wire_unauthorized(f: &Fixture, case: CredentialCase) -> Vec<u8> {
    let mut harness = wire(f).await;
    let mut request = b"POST /shamir/v1/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/msgpack\r\n".to_vec();
    if let Some(auth) = authorization(f, case).await {
        if matches!(case, CredentialCase::Multiple) {
            request.extend_from_slice(&auth);
        } else {
            request.extend_from_slice(b"Authorization: ");
            request.extend_from_slice(&auth);
        }
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"Content-Length: 0\r\n\r\n");
    harness
        .stream
        .write_all(&request)
        .await
        .expect("write request");
    let (head, body) =
        tokio::time::timeout(Duration::from_secs(2), read_response(&mut harness.stream))
            .await
            .expect("401 response");
    assert!(
        head.starts_with("HTTP/1.1 401"),
        "{case_name}: {head}",
        case_name = credential_name(case)
    );
    assert_eq!(body, GOLDEN_401_BODY, "{} body", credential_name(case));
    let normalized: Vec<_> = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
        .map(|(name, value)| {
            format!(
                "{}:{}",
                name.to_ascii_lowercase(),
                value.trim().to_ascii_lowercase()
            )
        })
        .collect();
    assert_eq!(
        normalized,
        ["www-authenticate:bearer"],
        "{} challenge",
        credential_name(case)
    );
    assert!(
        head.to_ascii_lowercase().contains("connection: close"),
        "{} must close: {head}",
        credential_name(case)
    );
    assert_eq!(harness.exec.calls.load(Ordering::SeqCst), 0);
    let mut extra = [0; 1];
    match tokio::time::timeout(Duration::from_secs(2), harness.stream.read(&mut extra))
        .await
        .expect("connection close")
    {
        Ok(0) => {}
        Ok(n) => panic!("unexpected bytes after 401: {n}"),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            ) => {}
        Err(error) => panic!("unexpected connection-close error: {error}"),
    }
    harness.shutdown.cancel();
    harness
        .server
        .await
        .expect("server task")
        .expect("serve result");
    body
}
async fn read_response(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut head = Vec::new();
    loop {
        let byte = tokio::time::timeout(Duration::from_secs(2), stream.read_u8())
            .await
            .expect("response head deadline")
            .expect("response head");
        head.push(byte);
        assert!(head.len() <= 4096, "response head exceeds bound");
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8(head).expect("HTTP response head");
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);
    assert!(length <= 1024, "response body exceeds bound");
    let mut body = vec![0; length];
    tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut body))
        .await
        .expect("response body deadline")
        .expect("response body");
    (head, body)
}

#[tokio::test]
async fn credential_failure_classes_have_uniform_real_wire_401_responses() {
    for case in [
        CredentialCase::Missing,
        CredentialCase::Multiple,
        CredentialCase::Malformed,
        CredentialCase::UnknownId,
        CredentialCase::WrongSecret,
        CredentialCase::Revoked,
        CredentialCase::Expired,
        CredentialCase::OwnerFloor,
    ] {
        let f = fixture().await;
        assert_wire_unauthorized(&f, case).await;
        f.gate.shutdown().await;
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn rest_authentication_tracing_redacts_credentials_and_secrets() {
    let f = fixture().await;
    let valid = token(&f, "redaction-check").await;
    let mut wrong = valid.as_bytes().to_vec();
    wrong[28] = if wrong[28] == b'a' { b'b' } else { b'a' };
    let wrong = String::from_utf8(wrong).expect("token ASCII");
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(capture.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    for candidate in [&valid, &wrong] {
        let header = format!("Bearer {candidate}");
        let peer = PeerInfo {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7340),
            tls: false,
            forwarded_for: Vec::new(),
        };
        let decision = f
            .gate
            .authenticate(AuthHeader::Single(header.as_bytes()), Endpoint::Rest, peer)
            .await;
        if candidate == &valid {
            assert!(matches!(decision, GateDecision::Allow(_)));
        } else {
            assert!(matches!(decision, GateDecision::Unauthorized));
        }
    }
    let logs =
        String::from_utf8(capture.0.lock().expect("capture lock").clone()).expect("UTF-8 logs");
    assert!(
        logs.contains("token authentication failed"),
        "expected fixed failure trace"
    );
    for secret in [&valid, &wrong] {
        assert!(!logs.contains(secret), "credential token leaked to trace");
        assert!(
            !logs.contains(&secret[secret.rfind('.').expect("secret segment") + 1..]),
            "token secret leaked to trace"
        );
    }
    f.gate.shutdown().await;
}
