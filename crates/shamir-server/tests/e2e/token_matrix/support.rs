use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use super::super::http_rest::common::Harness;
use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_query_types::wire::DbRequest;
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::extract_tls_exporter;

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
        .expect("parse certificate");
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("trust generated cert");
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
        .expect("request write timeout")
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
        if let Some(pos) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
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
        let read_len = (body_end - bytes.len()).min(buf.len());
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthChallenge {
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    #[serde(with = "serde_bytes")]
    server_nonce: Vec<u8>,
}
#[derive(Serialize)]
struct AuthInit {
    user: String,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
    version: u8,
}
#[derive(Serialize)]
struct ClientProof {
    #[serde(with = "serde_bytes")]
    client_proof: Vec<u8>,
}
#[derive(Deserialize)]
struct AuthOk {
    #[serde(with = "serde_bytes")]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
    #[serde(default)]
    _server_query_version: u8,
    #[serde(default)]
    _rotation_in_progress: Option<shamir_connect::common::wire::RotationInProgressWire>,
    #[serde(default)]
    _kdf_upgrade_required: Option<bool>,
}
pub(in crate::token_matrix) struct RawScram {
    read: tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    write: tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    session_id: [u8; 32],
    exporter: [u8; 32],
    next_rid: u32,
}
impl RawScram {
    pub(in crate::token_matrix) async fn connect(
        harness: &Harness,
        username: &str,
        password: &[u8],
    ) -> Self {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        let pem = std::fs::read(harness.temp.path().join("cert.pem")).expect("generated cert");
        let cert = rustls::pki_types::CertificateDer::pem_slice_iter(&pem)
            .next()
            .expect("certificate PEM")
            .expect("parse certificate");
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert).expect("trust generated cert");
        let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(std::sync::Arc::new(config));
        let tcp = tokio::time::timeout_at(deadline, TcpStream::connect(harness.native_addr()))
            .await
            .expect("SCRAM connect timeout")
            .expect("SCRAM TCP connect");
        let tls = tokio::time::timeout_at(
            deadline,
            connector.connect(
                rustls::pki_types::ServerName::try_from("localhost").expect("server name"),
                tcp,
            ),
        )
        .await
        .expect("SCRAM TLS timeout")
        .expect("SCRAM TLS handshake");
        let exporter = extract_tls_exporter(&tls).expect("TLS exporter");
        let handshake = HandshakeBuilder::new(
            NormalizedUsername::from_raw(username).expect("username"),
            TransportKind::Tcp,
            BindingMode::TlsExporter,
        )
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .expect("SCRAM handshake");
        let (mut read, mut write) = tokio::io::split(tls);
        let init = handshake.auth_init();
        let init = AuthInit {
            user: init.user,
            client_nonce: init.client_nonce.to_vec(),
            binding_mode: init.binding_mode,
            version: init.version,
        };
        let bytes = rmp_serde::to_vec(&init).expect("encode auth_init");
        tokio::time::timeout_at(deadline, write_frame(&mut write, &bytes))
            .await
            .expect("auth_init timeout")
            .expect("send auth_init");
        let bytes =
            tokio::time::timeout_at(deadline, read_frame(&mut read, MAX_FRAME_SIZE_DEFAULT))
                .await
                .expect("challenge timeout")
                .expect("read challenge");
        let challenge: AuthChallenge = rmp_serde::from_slice(&bytes).expect("decode challenge");
        let mut salt = [0; 16];
        salt.copy_from_slice(&challenge.salt);
        let mut server_nonce = [0; 32];
        server_nonce.copy_from_slice(&challenge.server_nonce);
        let challenge = ServerChallenge {
            salt,
            kdf_params: KdfParams {
                memory_kb: challenge.memory_kb,
                time: challenge.time,
                parallelism: challenge.parallelism,
                argon2_version: challenge.argon2_version,
            },
            server_nonce,
        };
        let mut password = Zeroizing::new(password.to_vec());
        let (proof, derived, am) = handshake
            .process_challenge(&challenge, &mut password)
            .expect("process challenge");
        let bytes = rmp_serde::to_vec(&ClientProof {
            client_proof: proof.to_vec(),
        })
        .expect("encode proof");
        tokio::time::timeout_at(deadline, write_frame(&mut write, &bytes))
            .await
            .expect("proof timeout")
            .expect("send proof");
        let bytes =
            tokio::time::timeout_at(deadline, read_frame(&mut read, MAX_FRAME_SIZE_DEFAULT))
                .await
                .expect("auth_ok timeout")
                .expect("read auth_ok");
        let ok: AuthOk = rmp_serde::from_slice(&bytes).expect("decode auth_ok");
        let mut signature = [0; 32];
        signature.copy_from_slice(&ok.server_signature);
        let mut public_key = [0; 32];
        public_key.copy_from_slice(&ok.server_pub_key);
        let mut identity_sig = [0; 64];
        identity_sig.copy_from_slice(&ok.identity_sig);
        let mut session_id = [0; 32];
        session_id.copy_from_slice(&ok.session_id);
        let auth_ok = ServerAuthOk {
            server_signature: signature,
            server_pub_key: public_key,
            identity_sig,
            session_id,
            expires_at_ns: ok.expires_at_ns,
            resumption_ticket: Some(ok.resumption_ticket),
            resumption_expires_at_ns: Some(ok.resumption_expires_at_ns),
            rotation_in_progress: None,
            kdf_upgrade_required: None,
        };
        handshake
            .process_auth_ok(&auth_ok, &derived, &am, |_pin| {})
            .expect("verify auth_ok");
        Self {
            read,
            write,
            session_id,
            exporter,
            next_rid: 1,
        }
    }

    pub(in crate::token_matrix) fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    pub(in crate::token_matrix) fn exporter(&self) -> [u8; 32] {
        self.exporter
    }

    pub(in crate::token_matrix) async fn request_raw(&mut self, request: &DbRequest) -> Vec<u8> {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        let rid = self.next_rid;
        self.next_rid = self.next_rid.checked_add(1).expect("request id overflow");
        let payload = rmp_serde::to_vec_named(request).expect("encode request");
        let envelope = RequestEnvelope::new(self.session_id, Some(rid), payload)
            .to_msgpack()
            .expect("encode envelope");
        tokio::time::timeout_at(deadline, write_frame(&mut self.write, &envelope))
            .await
            .expect("request write timeout")
            .expect("send request");
        let bytes =
            tokio::time::timeout_at(deadline, read_frame(&mut self.read, MAX_FRAME_SIZE_DEFAULT))
                .await
                .expect("response timeout")
                .expect("read response");
        let response = ResponseEnvelope::from_msgpack(&bytes).expect("decode response envelope");
        assert_eq!(response.request_id, Some(rid));
        response.res
    }
}
