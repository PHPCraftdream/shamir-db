//! Server-perimeter control for `DescribeRepo` over token transports
//! (R5-2b item 6d).
//!
//! The token shell refuses `DescribeRepo` in the CLIENT allowlist, and the
//! server refuses the same typed operation again (`token_request_denied`,
//! `shamir-server-security` `http_rest/scope.rs`). This control bypasses
//! every client-side filter: it speaks the HTTPS REST and token-WSS wire
//! formats directly with the same bearer token and asserts the SERVER
//! refusal on each endpoint exactly once. Removing the server refusal
//! turns the test red without touching the shell.
//!
//! The repo is proven to exist with a real admin SCRAM session first, so a
//! missing server denial cannot be masked by a missing-repo error. On both
//! endpoints the SAME token and the SAME socket first carry an allowed
//! Ping (positive control), so the refusal is attributable to the typed
//! operation, not the session.
//!
//! Fixture-only wire plumbing: TLS trust from the fixture CA (http/1.1
//! ALPN), bounded reads with body caps, validated `HeaderValue` only, and
//! zeroized request buffers — no token material in argv, logs or `Debug`
//! output, and no network data in panic text.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_builder::token_admin::{create_api_token, CreateApiToken};
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroizing;

use crate::shell_tests::harness::secure_fixture::{
    boot_secure, provision_token, scan_operator_home, secret_suffix, write_secure_profile,
    SecureFixture,
};
use crate::shell_tests::harness::{
    run_shell, seed_catalogue, seed_shell_user, ShellRun, DB_NAME, REPO_NAME, SHELL_USER,
};

/// Every awaited fixture step is absolutely bounded (a hang detector; the
/// CI Windows runner is up to 10x slower).
const BOUND: Duration = Duration::from_secs(120);
const MAX_HEAD: usize = 32 * 1024;
const MAX_BODY: usize = 64 * 1024;
const REST_REQUEST_PATH: &str = "/shamir/v1/request";
const REST_PING_PATH: &str = "/shamir/v1/ping";
const WSS_PATH: &str = "/shamir/v1/ws";
const WS_SUBPROTOCOL: &str = "shamir-v1";
const DENIED: &str = "token_request_denied";

fn token_request() -> CreateApiToken {
    create_api_token("server-perimeter")
        .for_user(SHELL_USER)
        .mode(ApiTokenMode::ReadOnly)
        .channels(ApiTokenChannels::Both)
}

/// Unscoped read-only token for the seeded shell user: the same principal
/// and Root ACL pass the perimeter database-scope check elsewhere, so the
/// expected denial can only be the typed-operation refusal, never a
/// database-scope denial that would mask it.
#[test]
fn server_refuses_describe_repo_on_token_transports() {
    let fixture = boot_secure("server_perimeter_describe_repo_refusal");
    let shell = &fixture.shell;
    seed_catalogue(shell);
    drop(seed_shell_user(shell));
    let token = provision_token(shell, token_request());

    // SCRAM existence proof: the SAME operation succeeds for the admin over
    // the pinned native WSS listener, so a later server denial is the
    // token-perimeter refusal and not a missing repo.
    write_secure_profile(shell, "wss", fixture.wss_addr, DB_NAME, None);
    let scram = run_shell(shell, &["repo", "describe", REPO_NAME]);
    assert_success(&scram, "repo describe", "DescribeRepo");
    let suffix = secret_suffix(&token);
    let password_suffix = secret_suffix(shell.canary.as_str());
    // Both streams of the post-SCRAM run are scanned for every needle: the
    // admin password, its suffix, the token and its suffix.
    for (needle, label) in [
        (shell.canary.as_str(), "password"),
        (password_suffix, "password suffix"),
        (token.as_str(), "token"),
        (suffix, "token suffix"),
    ] {
        assert!(
            !scram.contains_canary(needle),
            "{label} in SCRAM shell output"
        );
    }
    scan_operator_home(
        shell,
        &[
            shell.canary.as_str(),
            password_suffix,
            token.as_str(),
            suffix,
        ],
    );
    assert_described_repo(&scram);

    shell.guard.mark("https-rest");
    rest_control(&fixture, &token);
    shell.guard.mark("token-wss");
    wss_control(&fixture, &token);
}

fn assert_success(run: &ShellRun, command: &str, op: &str) {
    assert_eq!(run.exit_code, 0, "{command}: expected success");
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated stdout");
    assert!(!line.contains(&b'\n'), "expected a single stdout record");
    let value: serde_json::Value = serde_json::from_slice(line).expect("valid JSON record");
    assert!(value["version"].as_u64() == Some(1), "expected v1");
    assert!(
        value["command"].as_str() == Some(command),
        "wrong command ID"
    );
    assert!(value["kind"].as_str() == Some("result"), "expected result");
    assert!(value["op"].as_str() == Some(op), "wrong op");
    assert!(value.get("code").is_none() && value.get("message").is_none());
}

fn assert_described_repo(run: &ShellRun) {
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated stdout");
    let value: serde_json::Value = serde_json::from_slice(line).expect("valid JSON record");
    assert!(
        value["data"]["describe_repo"].as_str() == Some(REPO_NAME),
        "SCRAM describe did not confirm the fixture repo"
    );
}

fn describe_request() -> DbRequest {
    let mut batch = Batch::new();
    batch.describe_repo("describe", ddl::describe_repo(REPO_NAME));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB_NAME.into(),
        batch: batch.try_build().expect("typed describe_repo batch"),
    }
}

fn ping_request() -> DbRequest {
    DbRequest::Ping
}

/// The bearer header is built once through `HeaderValue::from_str`, which
/// rejects every invalid octet; only known fixed fields plus that
/// validated value are ever written to the wire. The header is marked
/// sensitive so no `Debug`/log path prints it, and the temporary composed
/// string is zeroized.
fn validated_bearer(token: &Zeroizing<String>) -> HeaderValue {
    let value = Zeroizing::new(format!("Bearer {}", token.as_str()));
    let mut header = HeaderValue::from_str(value.as_str()).expect("bearer header validates");
    header.set_sensitive(true);
    header
}

fn rest_head(method: &str, path: &str, auth: &HeaderValue, body_len: usize) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(
        format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: {}\r\n\
             Content-Type: application/msgpack\r\nContent-Length: {body_len}\r\n\
             Connection: close\r\n\r\n",
            auth.to_str().expect("validated header is visible ASCII"),
        )
        .into_bytes(),
    )
}

/// One HTTPS REST round: the same token first Pings (allowed, status 200),
/// then sends the typed `DescribeRepo` batch and must draw the exact 403
/// with the msgpack `token_request_denied` body code. No client allowlist
/// sits between this socket and the server perimeter.
fn rest_control(fixture: &SecureFixture, token: &Zeroizing<String>) {
    let auth = validated_bearer(token);
    let ping_head = rest_head("GET", REST_PING_PATH, &auth, 0);
    let body =
        Zeroizing::new(rmp_serde::to_vec_named(&describe_request()).expect("encode request"));
    let describe_head = rest_head("POST", REST_REQUEST_PATH, &auth, body.len());
    let addr = fixture.http_addr;
    let ca_path = fixture.ca_path.clone();
    fixture.shell.rt.run(async move {
        // Bounded inside the runtime job: no timer is touched before the
        // Tokio runtime is entered.
        tokio::time::timeout(BOUND, async move {
            let mut stream = tls_stream(addr, &ca_path).await;
            let mut request = Zeroizing::new(ping_head.as_slice().to_vec());
            tokio::time::timeout(BOUND, stream.write_all(request.as_slice()))
                .await
                .expect("ping write timeout")
                .unwrap_or_else(|_| panic!("ping write"));
            let (status, _) = read_rest_response(&mut stream).await;
            assert_eq!(status, 200, "same-token REST Ping must be allowed");

            let mut stream = tls_stream(addr, &ca_path).await;
            request.clear();
            request.extend_from_slice(describe_head.as_slice());
            request.extend_from_slice(body.as_slice());
            tokio::time::timeout(BOUND, stream.write_all(request.as_slice()))
                .await
                .expect("request write timeout")
                .unwrap_or_else(|_| panic!("request write"));
            let (status, body) = read_rest_response(&mut stream).await;
            assert_eq!(status, 403, "REST perimeter must answer 403");
            let Ok(response) = rmp_serde::from_slice::<DbResponse>(&body) else {
                panic!("REST refusal body must decode as DbResponse");
            };
            assert!(
                matches!(&response, DbResponse::Error { code, .. } if code == DENIED),
                "REST body must carry the typed {DENIED} refusal"
            );
        })
        .await
        .expect("raw REST control finishes within the fixture bound");
    });
}

async fn tls_stream(
    addr: std::net::SocketAddr,
    ca_path: &std::path::Path,
) -> TlsStream<tokio::net::TcpStream> {
    let pem = std::fs::read(ca_path).expect("fixture trust anchor");
    let cert = CertificateDer::pem_slice_iter(&pem)
        .next()
        .expect("certificate PEM")
        .expect("parse certificate");
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("trust the fixture CA");
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tcp = tokio::time::timeout(BOUND, tokio::net::TcpStream::connect(addr))
        .await
        .expect("TCP connect timeout")
        .unwrap_or_else(|_| panic!("TCP connect failed"));
    let tls = tokio::time::timeout(
        BOUND,
        TlsConnector::from(std::sync::Arc::new(config))
            .connect(ServerName::try_from("localhost").expect("server name"), tcp),
    )
    .await
    .expect("TLS handshake timeout")
    .unwrap_or_else(|_| panic!("TLS 1.3 handshake failed"));
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(&b"http/1.1"[..]),
        "server must negotiate http/1.1"
    );
    tls
}

/// Bounded HTTP/1.1 response read with head and body caps.
async fn read_rest_response(stream: &mut TlsStream<tokio::net::TcpStream>) -> (u16, Vec<u8>) {
    let deadline = tokio::time::Instant::now() + BOUND;
    let mut bytes = Vec::new();
    let header_end = loop {
        assert!(bytes.len() < MAX_HEAD, "HTTP response head too large");
        let mut buf = [0; 4096];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .expect("HTTP response head timed out")
            .unwrap_or_else(|_| panic!("HTTP response head read"));
        assert_ne!(n, 0, "connection ended before response head");
        bytes.extend_from_slice(&buf[..n]);
        if let Some(pos) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let end = pos + 4;
            assert!(end <= MAX_HEAD, "HTTP response head too large");
            break end;
        }
    };
    let head = String::from_utf8(bytes[..header_end].to_vec()).expect("response head UTF-8");
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .expect("HTTP status")
        .parse()
        .expect("numeric HTTP status");
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("numeric length"))
        })
        .unwrap_or(0);
    assert!(length <= MAX_BODY, "HTTP response body too large");
    let mut body = bytes[header_end..].to_vec();
    while body.len() < length {
        let mut buf = [0; 4096];
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .expect("HTTP response body timed out")
            .unwrap_or_else(|_| panic!("HTTP response body read"));
        assert_ne!(n, 0, "connection ended before response body");
        body.extend_from_slice(&buf[..n]);
    }
    (status, body)
}

/// One token-WSS socket: upgrade with the bearer token, the server binding
/// frame, then a Ping (allowed envelope) followed by the typed
/// `DescribeRepo` batch, whose envelope must carry the
/// `token_request_denied` error code.
fn wss_control(fixture: &SecureFixture, token: &Zeroizing<String>) {
    let auth = validated_bearer(token);
    let addr = fixture.http_addr;
    let ca_path = fixture.ca_path.clone();
    fixture.shell.rt.run(async move {
        tokio::time::timeout(BOUND, async move {
            let tls = tls_stream(addr, &ca_path).await;
            let url = format!("ws://localhost:{}{WSS_PATH}", addr.port());
            let mut request = url
                .into_client_request()
                .expect("websocket request URI builds");
            request.headers_mut().insert("Authorization", auth);
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static(WS_SUBPROTOCOL),
            );
            let (mut ws, response) = tokio::time::timeout(BOUND, client_async(request, tls))
                .await
                .expect("websocket handshake timeout")
                .unwrap_or_else(|_| panic!("websocket upgrade failed"));
            assert_eq!(response.status().as_u16(), 101, "expected 101 upgrade");
            let sid = read_binding(&mut ws).await;

            let ping = rmp_serde::to_vec_named(&ping_request()).expect("encode Ping");
            send_envelope(&mut ws, sid, 1, &ping).await;
            let reply = read_envelope(&mut ws, 1).await;
            assert!(
                matches!(reply, DbResponse::Pong),
                "same-token WSS Ping must be allowed"
            );

            let payload = Zeroizing::new(
                rmp_serde::to_vec_named(&describe_request()).expect("encode request"),
            );
            send_envelope(&mut ws, sid, 2, payload.as_slice()).await;
            let reply = read_envelope(&mut ws, 2).await;
            assert!(
                matches!(&reply, DbResponse::Error { code, .. } if code == DENIED),
                "WSS envelope must carry the typed {DENIED} refusal"
            );
        })
        .await
        .expect("raw WSS control finishes within the fixture bound");
    });
}

async fn send_envelope(
    ws: &mut WebSocketStream<TlsStream<tokio::net::TcpStream>>,
    sid: [u8; 32],
    rid: u32,
    req: &[u8],
) {
    let frame = RequestEnvelope::new(sid, Some(rid), req.to_vec())
        .to_msgpack()
        .expect("encode request envelope");
    tokio::time::timeout(BOUND, ws.send(Message::Binary(frame.into())))
        .await
        .expect("frame send timeout")
        .unwrap_or_else(|_| panic!("send request frame"));
}

/// Reads one binary frame and asserts it echoes the expected request ID,
/// so a swapped or replayed reply cannot pass for the awaited response.
async fn read_envelope(
    ws: &mut WebSocketStream<TlsStream<tokio::net::TcpStream>>,
    expected_rid: u32,
) -> DbResponse {
    loop {
        let message = tokio::time::timeout(BOUND, ws.next())
            .await
            .expect("response frame timeout")
            .unwrap_or_else(|| panic!("response frame present"))
            .unwrap_or_else(|_| panic!("response frame read"));
        let Message::Binary(bytes) = message else {
            continue;
        };
        let Ok(envelope) = ResponseEnvelope::from_msgpack(&bytes) else {
            panic!("response frame must decode as a ResponseEnvelope");
        };
        assert_eq!(
            envelope.request_id,
            Some(expected_rid),
            "response request ID mismatch"
        );
        let Ok(reply) = rmp_serde::from_slice::<DbResponse>(&envelope.res) else {
            panic!("envelope body must decode as DbResponse");
        };
        return reply;
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: ByteBuf,
    expires_at_ns: u64,
}

async fn read_binding<S>(ws: &mut WebSocketStream<S>) -> [u8; 32]
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let message = tokio::time::timeout(BOUND, ws.next())
            .await
            .expect("binding frame timeout")
            .expect("binding frame present")
            .expect("binding frame read");
        let Message::Binary(bytes) = message else {
            continue;
        };
        let Ok(binding) = rmp_serde::from_slice::<Binding>(&bytes) else {
            panic!("first server frame must decode as the binding frame");
        };
        assert!(binding.ok && binding.expires_at_ns > 0, "invalid binding");
        let Ok(sid) = <[u8; 32]>::try_from(binding.sid.as_slice()) else {
            panic!("binding sid must be 32 bytes");
        };
        return sid;
    }
}
