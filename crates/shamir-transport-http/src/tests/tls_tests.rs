use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use bytes::Bytes;
use rustls::{
    crypto::aws_lc_rs,
    pki_types::{pem::PemObject, CertificateDer, ServerName},
    RootCertStore,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

use crate::{
    serve, AuthHeader, BodyBudget, Endpoint, EndpointSet, ExecOutcome, ExecStatus, GateDecision,
    HttpLimits, HttpServerConfig, PeerInfo, RestExecutor, TokenGate,
};

struct Gate(AtomicUsize);
impl TokenGate for Gate {
    type Grant = ();
    fn authenticate<'a>(
        &'a self,
        _: AuthHeader<'a>,
        _: Endpoint,
        _: PeerInfo,
    ) -> impl Future<Output = GateDecision<()>> + Send + 'a {
        self.0.fetch_add(1, Ordering::SeqCst);
        async { GateDecision::Allow(()) }
    }
    fn still_valid(&self, _: &()) -> bool {
        true
    }
}
struct Executor(AtomicUsize);
impl RestExecutor<()> for Executor {
    fn execute<'a>(&'a self, _: &'a (), _: Bytes) -> impl Future<Output = ExecOutcome> + Send + 'a {
        self.0.fetch_add(1, Ordering::SeqCst);
        async {
            ExecOutcome {
                status: ExecStatus::Ok,
                body: Bytes::from_static(b"pong"),
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

type TlsHarness = (
    SocketAddr,
    Arc<Gate>,
    Arc<Executor>,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), crate::ServeError>>,
    Arc<rustls::ClientConfig>,
    RootCertStore,
);

async fn start(hsts: bool, alpn: Vec<Vec<u8>>) -> TlsHarness {
    let (cert_pem, key_pem) = tokio::task::spawn_blocking(|| {
        shamir_transport_tcp::tls::generate_self_signed_server_cert(vec!["localhost".into()])
    })
    .await
    .expect("certificate task")
    .expect("certificate");
    let mut server_tls =
        shamir_transport_tcp::tls::make_server_config_from_pem(&cert_pem, &key_pem)
            .expect("server config");
    Arc::get_mut(&mut server_tls)
        .expect("unique TLS config")
        .alpn_protocols = alpn;
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM");
    let mut roots = RootCertStore::empty();
    for cert in certs {
        roots.add(cert).expect("trusted generated certificate");
    }
    let client_tls =
        rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3 versions")
            .with_root_certificates(roots.clone())
            .with_no_client_auth();
    let mut client_tls = client_tls;
    client_tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let gate = Arc::new(Gate(AtomicUsize::new(0)));
    let exec = Arc::new(Executor(AtomicUsize::new(0)));
    let cfg = Arc::new(HttpServerConfig {
        limits: HttpLimits::new(1024, Arc::new(BodyBudget::new(4096))),
        endpoints: EndpointSet {
            rest: true,
            ws: false,
        },
        tls: Some(server_tls),
        hsts,
        forbidden_body: Arc::new(|_| Bytes::new()),
        ping_body: Bytes::new(),
    });
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(serve(
        listener,
        cfg,
        gate.clone(),
        exec.clone(),
        Arc::new(Admission),
        shutdown.clone(),
    ));
    (
        addr,
        gate,
        exec,
        shutdown,
        task,
        Arc::new(client_tls),
        roots,
    )
}

async fn response<S: AsyncReadExt + Unpin>(stream: &mut S) -> (String, Vec<u8>) {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.expect("response head");
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).expect("response headers");
    let len = text
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = vec![0; len];
    stream
        .read_exact(&mut body)
        .await
        .expect("complete response body");
    (text, body)
}
async fn stop(
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), crate::ServeError>>,
) {
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("server stops")
        .expect("serve task")
        .expect("serve result");
}

#[tokio::test]
async fn tls13_ping_uses_http11_and_hsts_only_when_enabled() {
    let (addr, gate, exec, shutdown, task, tls, _) =
        start(true, vec![b"h2".to_vec(), b"http/1.1".to_vec()]).await;
    let connector = TlsConnector::from(tls);
    let stream = connector
        .connect(
            ServerName::try_from("localhost").expect("server name"),
            TcpStream::connect(addr).await.expect("connect"),
        )
        .await
        .expect("TLS 1.3 handshake");
    assert_eq!(
        stream.get_ref().1.alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );
    assert_eq!(
        stream.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    let mut stream = stream;
    stream.write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer opaque\r\n\r\n").await.expect("ping");
    let (head, body) = tokio::time::timeout(Duration::from_secs(2), response(&mut stream))
        .await
        .expect("HTTP response");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(body, b"pong");
    assert!(head
        .to_ascii_lowercase()
        .contains("strict-transport-security: max-age=31536000"));
    assert!(head
        .to_ascii_lowercase()
        .contains("cache-control: no-store"));
    assert!(head
        .to_ascii_lowercase()
        .contains("x-content-type-options: nosniff"));
    assert!(!head.to_ascii_lowercase().contains("access-control-"));
    assert_eq!(gate.0.load(Ordering::SeqCst), 1);
    assert_eq!(exec.0.load(Ordering::SeqCst), 1);
    stop(shutdown, task).await;
}

#[tokio::test]
async fn tls13_without_hsts_omits_hsts_header() {
    let (addr, _, _, shutdown, task, tls, _) = start(false, vec![b"http/1.1".to_vec()]).await;
    let mut stream = TlsConnector::from(tls)
        .connect(
            ServerName::try_from("localhost").expect("server name"),
            TcpStream::connect(addr).await.expect("connect"),
        )
        .await
        .expect("TLS 1.3 handshake");
    assert_eq!(
        stream.get_ref().1.alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );
    stream
        .write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("ping");
    let (head, body) = tokio::time::timeout(Duration::from_secs(2), response(&mut stream))
        .await
        .expect("HTTP response");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(body, b"pong");
    assert!(!head
        .to_ascii_lowercase()
        .contains("strict-transport-security:"));
    assert!(!head.to_ascii_lowercase().contains("access-control-"));
    stop(shutdown, task).await;
}

#[tokio::test]
async fn plaintext_client_never_reaches_gate() {
    let (addr, gate, _, shutdown, task, _, _) = start(true, vec![b"http/1.1".to_vec()]).await;
    let mut socket = TcpStream::connect(addr).await.expect("connect");
    socket
        .write_all(b"GET /shamir/v1/ping HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("plaintext");
    let mut data = Vec::new();
    let read_result = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut data))
        .await
        .expect("plaintext connection is promptly dropped");
    match read_result {
        Ok(_) => assert!(
            !data.starts_with(b"HTTP/"),
            "TLS listener must not serve plaintext HTTP"
        ),
        Err(error) => assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ),
            "unexpected plaintext socket error: {error}"
        ),
    }
    assert_eq!(gate.0.load(Ordering::SeqCst), 0);
    stop(shutdown, task).await;
}

#[tokio::test]
async fn tls12_only_client_is_rejected() {
    let (addr, gate, _, shutdown, task, _, roots) = start(false, vec![b"http/1.1".to_vec()]).await;
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .expect("TLS 1.2 versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        connector.connect(
            ServerName::try_from("localhost").expect("name"),
            TcpStream::connect(addr).await.expect("connect"),
        ),
    )
    .await
    .expect("handshake ends");
    assert!(result.is_err());
    assert_eq!(gate.0.load(Ordering::SeqCst), 0);
    stop(shutdown, task).await;
}
