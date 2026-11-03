use std::time::Duration;

use rustls::pki_types::pem::PemObject;

use super::super::http_rest::common::Harness;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

pub(super) trait Io: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> Io for T {}

pub(super) type Stream = Box<dyn Io + Unpin + Send>;

pub(super) const DEADLINE: Duration = Duration::from_secs(15);
pub(super) const MAX_HEAD: usize = 32 * 1024;
pub(super) const MAX_BODY: usize = 16 * 1024 * 1024;

pub(super) async fn connect(harness: &Harness, tls: bool) -> Stream {
    let tcp = tokio::time::timeout(DEADLINE, TcpStream::connect(harness.addr()))
        .await
        .expect("socket connect timed out")
        .expect("socket connect");
    if !tls {
        return Box::new(tcp);
    }
    Box::new(
        tokio::time::timeout(
            DEADLINE,
            tls_connector(harness).connect(
                rustls::pki_types::ServerName::try_from("localhost").expect("server name"),
                tcp,
            ),
        )
        .await
        .expect("TLS handshake timed out")
        .expect("TLS 1.3 handshake"),
    )
}

pub(super) fn tls_connector(harness: &Harness) -> TlsConnector {
    let pem = std::fs::read(harness.temp.path().join("cert.pem")).expect("generated cert");
    let cert = rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
        .next()
        .expect("certificate PEM")
        .expect("parse generated certificate");
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("trust generated certificate");
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConnector::from(std::sync::Arc::new(config))
}

pub(super) async fn send_http(harness: &Harness, tls: bool, request: &str) -> (String, Vec<u8>) {
    let mut stream = connect(harness, tls).await;
    tokio::time::timeout(DEADLINE, stream.write_all(request.as_bytes()))
        .await
        .expect("request write timed out")
        .expect("request write");
    read_response(&mut stream).await
}

pub(super) async fn read_response(stream: &mut Stream) -> (String, Vec<u8>) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut bytes = Vec::new();
    let header_end = loop {
        assert!(bytes.len() < MAX_HEAD, "HTTP response head too large");
        let mut buf = [0; 4096];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .expect("HTTP response head timed out")
            .expect("HTTP response head read");
        assert_ne!(n, 0, "connection ended before response head");
        bytes.extend_from_slice(&buf[..n]);
        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let end = pos + 4;
            assert!(end <= MAX_HEAD, "HTTP response head too large");
            break end;
        }
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).expect("response head UTF-8");
    let length = content_length(&head).unwrap_or(0);
    assert!(length <= MAX_BODY, "HTTP response body too large");
    let body_end = header_end
        .checked_add(length)
        .expect("HTTP response body length overflow");
    assert!(body_end <= MAX_HEAD + MAX_BODY, "HTTP response too large");
    while bytes.len() < body_end {
        let mut buf = [0; 4096];
        let remaining = body_end - bytes.len();
        let read_len = remaining.min(buf.len());
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf[..read_len]))
            .await
            .expect("HTTP response body timed out")
            .expect("HTTP response body read");
        assert_ne!(n, 0, "connection ended before response body");
        bytes.extend_from_slice(&buf[..n]);
    }
    (head, bytes[header_end..body_end].to_vec())
}

pub(super) fn status(head: &str) -> u16 {
    head.split_whitespace()
        .nth(1)
        .expect("HTTP status")
        .parse()
        .expect("numeric HTTP status")
}

pub(super) fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().expect("numeric Content-Length"))
    })
}
