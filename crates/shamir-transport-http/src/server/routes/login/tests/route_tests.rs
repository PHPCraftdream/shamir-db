use super::super::route_handler::{handle_login, login_response};
use super::super::{LoginHead, LoginResponse, LoginRoutes, NoLogin};
use crate::gate::PeerInfo;
use bytes::Bytes;
use http::{header, HeaderMap, StatusCode};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::{sync::Arc, time::Duration};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

struct DropSignal(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn response_extension_hold_survives_body_poll_until_owner_drop() {
    let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let response = LoginResponse::new(StatusCode::OK, Bytes::from_static(b"payload"))
        .with_hold(Arc::new(DropSignal(Arc::clone(&drops))));
    let response = login_response(response, false, false);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    let (parts, body) = response.into_parts();
    let _ = body.collect().await.expect("collect body");
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    drop(parts);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn response_hold_is_owned_until_taken_or_response_drop() {
    let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hold = Arc::new(DropSignal(Arc::clone(&drops)));
    let response = LoginResponse::new(StatusCode::OK, Bytes::new()).with_hold(hold);
    let mut response = response;
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    drop(response.take_hold());
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);

    let response = LoginResponse::new(StatusCode::OK, Bytes::new())
        .with_hold(Arc::new(DropSignal(Arc::clone(&drops))));
    drop(response);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 2);
}

struct Admit(Arc<std::sync::atomic::AtomicUsize>);
impl LoginRoutes for Admit {
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
    async fn handle(&self, _: Bytes, _: Arc<Self::Permit>, body: Bytes) -> LoginResponse {
        LoginResponse::new(StatusCode::OK, body)
    }
    fn refused(&self, _: &[u8], _: StatusCode) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn body_limit(&self, route: &[u8]) -> usize {
        if route.ends_with(b"finish") {
            256
        } else {
            4096
        }
    }
}

struct Deny;
impl LoginRoutes for Deny {
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
        Err(LoginResponse::new(
            StatusCode::FORBIDDEN,
            Bytes::from_static(b"deny"),
        ))
    }
    async fn handle(&self, _: Bytes, _: Arc<Self::Permit>, _: Bytes) -> LoginResponse {
        LoginResponse::new(StatusCode::OK, Bytes::new())
    }
    fn body_limit(&self, route: &[u8]) -> usize {
        if route.ends_with(b"finish") {
            256
        } else {
            4096
        }
    }
}

async fn exchange<L: LoginRoutes>(wire: &[u8], hook: Arc<L>) -> Vec<u8> {
    exchange_with_override(wire, hook, Arc::new(std::sync::atomic::AtomicU64::new(0))).await
}

async fn exchange_with_override<L: LoginRoutes>(
    wire: &[u8],
    hook: Arc<L>,
    write_timeout_override: Arc<std::sync::atomic::AtomicU64>,
) -> Vec<u8> {
    let (mut client, server) = duplex(8192);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    let peer = PeerInfo {
        addr: "127.0.0.1:1".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let service = service_fn(move |request| {
        let hook = hook.clone();
        let peer = peer.clone();
        let write_timeout_override = write_timeout_override.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                handle_login(
                    request,
                    hook,
                    peer,
                    false,
                    tokio::time::Instant::now(),
                    Duration::from_secs(20),
                    write_timeout_override.clone(),
                )
                .await,
            )
        }
    });
    drop(hold_tx);
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, hold_rx,
    ));
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    client.write_all(wire).await.expect("write request");
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut output))
        .await
        .unwrap()
        .expect("read response");
    task.abort();
    let _ = task.await;
    output
}

#[tokio::test]
async fn refusal_hook_counts_each_transport_body_refusal_once() {
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook = Arc::new(Admit(Arc::clone(&count)));
    let output = exchange(
        b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Length: 4097\r\n\r\n",
        hook,
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 413"));
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn no_login_is_404_without_auth() {
    let output = exchange(
        b"PATCH /shamir/v1/auth/finish HTTP/1.1\r\nHost: x\r\n\r\n",
        Arc::new(NoLogin),
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 404"));
    assert!(output.ends_with(b"\x81\xa5error\xa9not_found"));
}
#[tokio::test]
async fn method_rejected_with_allow_post() {
    let output = exchange(
        b"GET /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\n\r\n",
        Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 405"));
    assert!(output
        .windows(b"Allow: POST".len())
        .any(|w| w.eq_ignore_ascii_case(b"Allow: POST")));
}

#[tokio::test]
async fn route_arms_five_second_write_timeout_override() {
    let write_timeout_override = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let output = exchange_with_override(
        b"GET /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\n\r\n",
        Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
        write_timeout_override.clone(),
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 405"));
    assert_eq!(
        write_timeout_override.load(std::sync::atomic::Ordering::Acquire),
        5_000
    );
}
#[tokio::test]
async fn media_type_rejected_before_admission() {
    let output = exchange(
        b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack; charset=utf-8\r\nContent-Length: 0\r\n\r\n",
        Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 415"));
}

#[tokio::test]
async fn missing_or_duplicate_content_type_is_unsupported() {
    for wire in [
        &b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"[..],
        &b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n"[..],
    ] {
        let output = exchange(wire, Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0))))).await;
        assert!(output.starts_with(b"HTTP/1.1 415"));
        assert!(output.ends_with(b"\x81\xa5error\xb6unsupported_media_type"));
    }
}
#[tokio::test]
async fn content_encoding_rejected_even_when_empty() {
    let output = exchange(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Encoding:\r\nContent-Length: 0\r\n\r\n", Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0))))).await;
    assert!(output.starts_with(b"HTTP/1.1 415"));
}
#[tokio::test]
async fn declared_oversize_rejected_before_body_poll() {
    let output = exchange(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Length: 4097\r\n\r\n", Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0))))).await;
    assert!(output.starts_with(b"HTTP/1.1 413"));
}
#[tokio::test]
async fn head_rejection_does_not_send_continue_before_admission() {
    let output = exchange(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\n", Arc::new(Deny)).await;
    assert!(output.starts_with(b"HTTP/1.1 403"));
    assert!(!output
        .windows(b"HTTP/1.1 100 Continue\r\n\r\n".len())
        .any(|w| w == b"HTTP/1.1 100 Continue\r\n\r\n"));
}
#[tokio::test]
async fn streamed_oversize_body_is_capped() {
    let mut wire = b"POST /shamir/v1/auth/finish HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nTransfer-Encoding: chunked\r\n\r\n101\r\n".to_vec();
    wire.extend(std::iter::repeat_n(b'x', 257));
    wire.extend_from_slice(b"\r\n0\r\n\r\n");
    let output = exchange(
        &wire,
        Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 413"));
}
#[tokio::test(start_paused = true)]
async fn stalled_body_times_out_after_three_seconds() {
    let started = tokio::time::Instant::now();
    let (mut client, server) = duplex(8192);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    drop(hold_tx);
    let peer = PeerInfo {
        addr: "127.0.0.1:1".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let service = service_fn(move |request| {
        let peer = peer.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                handle_login(
                    request,
                    Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
                    peer,
                    false,
                    tokio::time::Instant::now(),
                    Duration::from_secs(20),
                    Arc::new(std::sync::atomic::AtomicU64::new(0)),
                )
                .await,
            )
        }
    });
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, hold_rx,
    ));
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    client.write_all(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\nda").await.unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(output.starts_with(b"HTTP/1.1 408"));
    assert!(output.ends_with(b"\x81\xa5error\xafrequest_timeout"));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(4),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn admitted_request_receives_continue_before_body_read() {
    let (mut client, server) = duplex(8192);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    drop(hold_tx);
    let peer = PeerInfo {
        addr: "127.0.0.1:1".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let service = service_fn(move |request| {
        let peer = peer.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                handle_login(
                    request,
                    Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
                    peer,
                    false,
                    tokio::time::Instant::now(),
                    Duration::from_secs(20),
                    Arc::new(std::sync::atomic::AtomicU64::new(0)),
                )
                .await,
            )
        }
    });
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, hold_rx,
    ));
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    client.write_all(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\n").await.unwrap();
    let mut interim = [0_u8; 25];
    tokio::time::timeout(Duration::from_secs(15), client.read_exact(&mut interim))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"data").await.unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(output.starts_with(b"HTTP/1.1 200"));
    assert!(output.ends_with(b"data"));
}

#[tokio::test]
async fn admitted_request_reads_body_and_preserves_exact_bytes() {
    let output = exchange(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\ndata", Arc::new(Admit(Arc::new(std::sync::atomic::AtomicUsize::new(0))))).await;
    assert!(output.windows(4).any(|w| w == b"data"));
}
#[test]
fn duplicate_header_values_remain_visible_as_bytes() {
    let mut headers = HeaderMap::new();
    headers.append(header::CONTENT_ENCODING, "gzip".parse().expect("header"));
    headers.append(header::CONTENT_ENCODING, "br".parse().expect("header"));
    let head = LoginHead::new(b"POST", b"/shamir/v1/auth/begin", &headers);
    assert_eq!(
        head.header_values(b"content-encoding").collect::<Vec<_>>(),
        vec![b"gzip".as_slice(), b"br".as_slice()]
    );
}
#[test]
fn login_response_contains_mandatory_headers_hsts_allow_and_no_credentials() {
    let reply = LoginResponse::new(
        StatusCode::METHOD_NOT_ALLOWED,
        Bytes::from_static(b"opaque"),
    );
    let response = login_response(reply, true, true);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/msgpack"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::PRAGMA], "no-cache");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()[header::CONNECTION], "close");
    assert_eq!(response.headers()[header::ALLOW], "POST");
    assert_eq!(
        response.headers()["strict-transport-security"],
        "max-age=31536000"
    );
    assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert!(!response
        .headers()
        .contains_key("access-control-allow-origin"));
}
#[test]
fn login_response_omits_hsts_without_tls() {
    let response = login_response(
        LoginResponse::new(StatusCode::OK, Bytes::new()),
        false,
        true,
    );
    assert!(!response.headers().contains_key("strict-transport-security"));
}
