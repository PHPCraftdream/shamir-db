use futures_util::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use crate::browser::BrowserOriginPolicy;
use crate::framing::MAX_WS_FRAME_SIZE;
use crate::server::{
    accept_browser_ws, accept_native_ws, server_ws_config, WsAcceptError, NATIVE_WS_PATH,
};
use tokio_tungstenite::tungstenite::handshake::client::{generate_key, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

/// NEW-1: the server accept config MUST cap incoming buffering at the
/// 16 MiB frame ceiling, strictly below tungstenite's 64 MiB
/// `max_message_size` default. Otherwise an unauthenticated WSS peer can
/// force ~64 MiB of buffering per connection before the framing layer's
/// 4 KiB pre-auth logical check runs.
#[test]
fn server_config_caps_message_and_frame_at_16_mib() {
    let cfg = server_ws_config();
    assert_eq!(
        cfg.max_message_size,
        Some(MAX_WS_FRAME_SIZE),
        "max_message_size must be pinned to the 16 MiB frame ceiling",
    );
    assert_eq!(
        cfg.max_frame_size,
        Some(MAX_WS_FRAME_SIZE),
        "max_frame_size must be pinned to the 16 MiB frame ceiling",
    );

    // Strictly below the tungstenite defaults (msg 64 MiB / frame 16 MiB).
    let defaults = WebSocketConfig::default();
    assert!(
        cfg.max_message_size < defaults.max_message_size,
        "server max_message_size ({:?}) must be below the 64 MiB default ({:?})",
        cfg.max_message_size,
        defaults.max_message_size,
    );
    assert!(
        cfg.max_message_size <= Some(16 * 1024 * 1024),
        "server max_message_size must be <= 16 MiB",
    );
}

/// NEW-1 (live wiring): a real WS handshake against `accept_native_ws`
/// must succeed AND the negotiated stream must reject a single message
/// that exceeds the 16 MiB cap — proving `server_ws_config` is actually
/// applied to the live accept path, not just returned by the helper.
///
/// The client is configured with no frame/message cap so the oversized
/// payload reaches the wire as one frame; the *server's* cap must be the
/// thing that rejects it.
#[tokio::test]
async fn live_accept_rejects_message_over_cap() {
    let (server_io, client_io) = duplex(64 * 1024 * 1024);

    let server = tokio::spawn(async move {
        let mut ws = accept_native_ws(server_io, NATIVE_WS_PATH, &BrowserOriginPolicy::empty())
            .await
            .expect("accept");
        // Reading the oversized message must error (Capacity), never
        // yield a 16 MiB+ buffered payload.
        match ws.next().await {
            Some(Err(e)) => format!("err: {e}"),
            Some(Ok(m)) => panic!("expected error, got message len {}", m.len()),
            None => "closed".to_string(),
        }
    });

    // Client side: uncapped so it emits the oversized frame verbatim.
    let client_cfg = WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None);
    let (mut client, _resp) = tokio_tungstenite::client_async_with_config(
        ws_request(NATIVE_WS_PATH, None),
        client_io,
        Some(client_cfg),
    )
    .await
    .expect("client handshake");

    // One byte over the server's 16 MiB ceiling.
    let oversized = vec![0xa5u8; MAX_WS_FRAME_SIZE + 1];
    // The send itself may surface the peer's protocol close; either way
    // the server task must report an error rather than a buffered body.
    let _ = client.send(Message::Binary(oversized.into())).await;

    let outcome = server.await.expect("server task join");
    assert!(
        outcome.starts_with("err:") || outcome == "closed",
        "server must reject the oversized message (got {outcome})",
    );
}

/// NEW-1 (live wiring, positive): a normal small frame still round-trips
/// through the capped accept path — the cap must not break legitimate
/// traffic.
#[tokio::test]
async fn live_accept_passes_small_frame() {
    let (server_io, client_io) = duplex(64 * 1024);

    let server = tokio::spawn(async move {
        let mut ws = accept_native_ws(server_io, NATIVE_WS_PATH, &BrowserOriginPolicy::empty())
            .await
            .expect("accept");
        match ws.next().await {
            Some(Ok(Message::Binary(b))) => b,
            other => panic!("expected binary, got {other:?}"),
        }
    });

    let (mut client, _resp) =
        tokio_tungstenite::client_async(ws_request(NATIVE_WS_PATH, None), client_io)
            .await
            .expect("client handshake");
    client
        .send(Message::Binary(b"hello".to_vec().into()))
        .await
        .expect("send");

    let got = server.await.expect("join");
    assert_eq!(&got[..], b"hello");
}

// ---------------------------------------------------------------------------
// Regression tests: RV-TRANSPORT-WS-F54426DC / F5B1F757 / 4FC6BCDD
// ---------------------------------------------------------------------------

/// Build a WS upgrade request with an explicit path and optional `Origin`.
/// Offers the mandatory `shamir-v1` subprotocol by default.
fn ws_request(path: &str, origin: Option<&str>) -> Request {
    ws_request_protocols(path, origin, Some("shamir-v1"))
}

/// Build a WS upgrade request with an explicit path and one raw header
/// (pass `None` to omit it — needed for non-`&str` header values such as
/// obs-text Origin bytes). Offers `shamir-v1` by default.
fn ws_request_header(path: &str, name: &str, value: Option<http::HeaderValue>) -> Request {
    let mut req = ws_request_protocols(path, None, Some("shamir-v1"));
    if let Some(v) = value {
        let (mut parts, body) = req.into_parts();
        parts.headers.insert(
            http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
            v,
        );
        req = Request::from_parts(parts, body);
    }
    req
}

/// Build a WS upgrade request; sets `Sec-WebSocket-Protocol` only when
/// `protocols` is `Some` (comma-separated, as browsers send it).
fn ws_request_protocols(path: &str, origin: Option<&str>, protocols: Option<&str>) -> Request {
    let mut builder = Request::builder()
        .uri(format!("ws://localhost{path}"))
        .header("Host", "localhost")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key());
    if let Some(o) = origin {
        builder = builder.header(
            "Origin",
            http::HeaderValue::from_str(o).expect("valid origin header"),
        );
    }
    if let Some(p) = protocols {
        builder = builder.header("Sec-WebSocket-Protocol", p);
    }
    builder.body(()).expect("valid request")
}

/// RV-TRANSPORT-WS-F54426DC: a native-WS client that SENDS an Origin header
/// not on the allowlist must be rejected with HTTP 403 during the
/// handshake (spec §9: Origin optional, but if present it must match).
#[tokio::test]
async fn rv_transport_ws_f54426dc_native_rejects_disallowed_origin() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);

    let server = tokio::spawn(async move {
        let res = accept_native_ws(server_io, NATIVE_WS_PATH, &policy).await;
        match res {
            Err(e) => format!("err: {e}"),
            Ok(_) => "accepted".to_string(),
        }
    });

    let req = ws_request(NATIVE_WS_PATH, Some("https://evil.example"));
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        }
        other => panic!("expected Http error, got {other:?}"),
    }

    let outcome = server.await.expect("join");
    assert!(outcome.starts_with("err:"), "server must reject: {outcome}");
}

/// RV-TRANSPORT-WS-F54426DC: a native-WS client that sends a SUPPLIED
/// Origin header whose bytes are not valid visible ASCII (`to_str()` fails)
/// must be rejected with 403 — a non-text Origin must never be treated as
/// absent (spec §9).
#[tokio::test]
async fn rv_transport_ws_f54426dc_native_rejects_non_text_origin() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);

    let server = tokio::spawn(async move {
        let res = accept_native_ws(server_io, NATIVE_WS_PATH, &policy).await;
        match res {
            Err(e) => format!("err: {e}"),
            Ok(_) => "accepted".to_string(),
        }
    });

    let origin = http::HeaderValue::from_bytes(b"https://evil.example/\xff")
        .expect("HeaderValue accepts obs-text bytes");
    let req = ws_request_header(NATIVE_WS_PATH, "Origin", Some(origin));
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        }
        other => panic!("expected Http error, got {other:?}"),
    }

    let outcome = server.await.expect("join");
    assert!(outcome.starts_with("err:"), "server must reject: {outcome}");
}

/// RV-TRANSPORT-WS-F54426DC (positive): a native client with an ALLOWED
/// Origin (or no Origin at all) completes the handshake.
#[tokio::test]
async fn rv_transport_ws_f54426dc_native_accepts_allowed_origin() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);

    let server = tokio::spawn(async move {
        accept_native_ws(server_io, NATIVE_WS_PATH, &policy)
            .await
            .expect("accept")
    });

    let req = ws_request(NATIVE_WS_PATH, Some("https://good.example"));
    let (_client, _resp) = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect("client handshake");
    server.await.expect("join");
}

/// RV-TRANSPORT-WS-F5B1F757: the accept path must use the CONFIGURED
/// endpoint path, not a hardcoded literal. A listener configured with
/// "/custom/v1" accepts clients on that path and rejects the old literal
/// "/shamir/v1" with 404; WrongPath carries the expected path.
#[tokio::test]
async fn rv_transport_ws_f5b1f757_configured_path_is_enforced() {
    let policy = BrowserOriginPolicy::empty();

    // Sub-case 1: matching custom path succeeds.
    let (server_io, client_io) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        accept_native_ws(server_io, "/custom/v1", &policy)
            .await
            .expect("accept")
    });
    let req = ws_request("/custom/v1", None);
    tokio_tungstenite::client_async(req, client_io)
        .await
        .expect("client handshake");
    server.await.expect("join");

    // Sub-case 2: the old hardcoded literal is now rejected with 404.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy2 = BrowserOriginPolicy::empty();
    let server =
        tokio::spawn(async move { accept_native_ws(server_io, "/custom/v1", &policy2).await });
    let req = ws_request(NATIVE_WS_PATH, None);
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }
        other => panic!("expected Http error, got {other:?}"),
    }
    match server.await.expect("join") {
        Err(WsAcceptError::WrongPath { expected, actual }) => {
            assert_eq!(expected, "/custom/v1");
            assert_eq!(actual, NATIVE_WS_PATH);
        }
        other => panic!("expected WrongPath, got {other:?}"),
    }
}

/// RV-TRANSPORT-WS-4FC6BCDD (a): browser endpoint, Origin missing → HTTP
/// 400 BAD_REQUEST during the handshake (spec §9: browser endpoint
/// requires Origin).
#[tokio::test]
async fn rv_transport_ws_4fc6bcdd_browser_missing_origin_rejected_400() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);

    let server = tokio::spawn(async move {
        accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy).await
    });

    let req = ws_request(crate::server::BROWSER_WS_PATH, None);
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
        other => panic!("expected Http error, got {other:?}"),
    }
    assert!(server.await.expect("join").is_err());
}

/// RV-TRANSPORT-WS-4FC6BCDD (b): browser endpoint, Origin present but not
/// on the allowlist → HTTP 403 FORBIDDEN.
#[tokio::test]
async fn rv_transport_ws_4fc6bcdd_browser_denied_origin_rejected_403() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);

    let server = tokio::spawn(async move {
        accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy).await
    });

    let req = ws_request(crate::server::BROWSER_WS_PATH, Some("https://evil.example"));
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        }
        other => panic!("expected Http error, got {other:?}"),
    }
    assert!(server.await.expect("join").is_err());
}

/// RV-TRANSPORT-WS-4FC6BCDD (c): browser endpoint, wrong path → HTTP 404.
#[tokio::test]
async fn rv_transport_ws_4fc6bcdd_browser_wrong_path_rejected_404() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);

    let server = tokio::spawn(async move {
        accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy).await
    });

    let req = ws_request(NATIVE_WS_PATH, Some("https://good.example"));
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }
        other => panic!("expected Http error, got {other:?}"),
    }
    assert!(server.await.expect("join").is_err());
}

// ---------------------------------------------------------------------------
// Regression tests: RV-TRANSPORT-WS-C0C43A18 (`shamir-v1` subprotocol)
// ---------------------------------------------------------------------------

/// Assert the 101 response selected `shamir-v1`.
fn assert_selected(resp: &Response) {
    assert_eq!(
        resp.headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok()),
        Some(crate::server::WS_SUBPROTOCOL),
        "handshake response must select the shamir-v1 subprotocol",
    );
}

/// Assert the handshake failed with `Error::Http` status 400.
async fn assert_rejected_400(req: Request, client_io: tokio::io::DuplexStream) {
    let err = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect_err("handshake must fail");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
        other => panic!("expected Http error, got {other:?}"),
    }
}

/// RV-TRANSPORT-WS-C0C43A18 (native): a client offering `shamir-v1` —
/// alone or among other protocols — gets a 101 whose response selects it.
#[tokio::test]
async fn rv_transport_ws_c0c43a18_native_offered_subprotocol_selected() {
    // Sub-case 1: `shamir-v1` alone.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::empty();
    let server = tokio::spawn(async move {
        accept_native_ws(server_io, NATIVE_WS_PATH, &policy)
            .await
            .expect("accept")
    });
    let req = ws_request_protocols(NATIVE_WS_PATH, None, Some("shamir-v1"));
    let (_client, resp) = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect("client handshake");
    assert_selected(&resp);
    server.await.expect("join");

    // Sub-case 2: `shamir-v1` among other offers.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::empty();
    let server = tokio::spawn(async move {
        accept_native_ws(server_io, NATIVE_WS_PATH, &policy)
            .await
            .expect("accept")
    });
    let req = ws_request_protocols(NATIVE_WS_PATH, None, Some("foo, shamir-v1"));
    let (_client, resp) = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect("client handshake");
    assert_selected(&resp);
    server.await.expect("join");
}

/// RV-TRANSPORT-WS-C0C43A18 (browser): same negotiation on
/// `accept_browser_ws` with a valid `Origin`.
#[tokio::test]
async fn rv_transport_ws_c0c43a18_browser_offered_subprotocol_selected() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);
    let server = tokio::spawn(async move {
        accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy)
            .await
            .expect("accept")
    });
    let req = ws_request_protocols(
        crate::server::BROWSER_WS_PATH,
        Some("https://good.example"),
        Some("foo, shamir-v1"),
    );
    let (_client, resp) = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect("client handshake");
    assert_selected(&resp);
    server.await.expect("join");
}

/// RV-TRANSPORT-WS-C0C43A18 (browser): `shamir-v1` offered ALONE is selected.
#[tokio::test]
async fn rv_transport_ws_c0c43a18_browser_single_protocol_selected() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);
    let server = tokio::spawn(async move {
        accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy)
            .await
            .expect("accept")
    });
    let req = ws_request_protocols(
        crate::server::BROWSER_WS_PATH,
        Some("https://good.example"),
        Some("shamir-v1"),
    );
    let (_client, resp) = tokio_tungstenite::client_async(req, client_io)
        .await
        .expect("client handshake");
    assert_selected(&resp);
    server.await.expect("join");
}

/// RV-TRANSPORT-WS-C0C43A18: a request with NO `Sec-WebSocket-Protocol`
/// header must be rejected with 400 on both acceptors.
#[tokio::test]
async fn rv_transport_ws_c0c43a18_missing_subprotocol_rejected_400() {
    // Native acceptor.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::empty();
    let server = tokio::spawn(async move {
        assert!(accept_native_ws(server_io, NATIVE_WS_PATH, &policy)
            .await
            .is_err());
    });
    let req = ws_request_protocols(NATIVE_WS_PATH, None, None);
    assert_rejected_400(req, client_io).await;
    server.await.expect("join");

    // Browser acceptor.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);
    let server = tokio::spawn(async move {
        assert!(
            accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy)
                .await
                .is_err()
        );
    });
    let req = ws_request_protocols(
        crate::server::BROWSER_WS_PATH,
        Some("https://good.example"),
        None,
    );
    assert_rejected_400(req, client_io).await;
    server.await.expect("join");
}

/// RV-TRANSPORT-WS-C0C43A18: offering only incompatible protocols must be
/// rejected with 400 on both acceptors.
#[tokio::test]
async fn rv_transport_ws_c0c43a18_only_incompatible_offers_rejected_400() {
    // Native acceptor.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::empty();
    let server = tokio::spawn(async move {
        assert!(accept_native_ws(server_io, NATIVE_WS_PATH, &policy)
            .await
            .is_err());
    });
    let req = ws_request_protocols(NATIVE_WS_PATH, None, Some("foo, bar"));
    assert_rejected_400(req, client_io).await;
    server.await.expect("join");

    // Browser acceptor.
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::allow(["https://good.example"]);
    let server = tokio::spawn(async move {
        assert!(
            accept_browser_ws(server_io, crate::server::BROWSER_WS_PATH, &policy)
                .await
                .is_err()
        );
    });
    let req = ws_request_protocols(
        crate::server::BROWSER_WS_PATH,
        Some("https://good.example"),
        Some("foo, bar"),
    );
    assert_rejected_400(req, client_io).await;
    server.await.expect("join");
}

/// RV-TRANSPORT-WS-C0C43A18: a `Sec-WebSocket-Protocol` header whose value
/// is not valid text (obs-text bytes) must be rejected with 400, never
/// treated as a no-match.
#[tokio::test]
async fn rv_transport_ws_c0c43a18_non_text_subprotocol_header_rejected_400() {
    let (server_io, client_io) = duplex(64 * 1024);
    let policy = BrowserOriginPolicy::empty();
    let server = tokio::spawn(async move {
        assert!(accept_native_ws(server_io, NATIVE_WS_PATH, &policy)
            .await
            .is_err());
    });
    // The tungstenite CLIENT refuses to build a non-text header value, so send
    // the handshake as raw bytes and read the raw server response.
    let mut client_io = client_io;
    let mut request = format!(
        "GET {NATIVE_WS_PATH} HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: "
    )
    .into_bytes();
    request.extend_from_slice(b"shamir-v1\xff\r\n\r\n");
    tokio::io::AsyncWriteExt::write_all(&mut client_io, &request)
        .await
        .expect("write raw handshake");
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::io::AsyncReadExt::read_to_end(&mut client_io, &mut response),
    )
    .await
    .expect("server must answer and close")
    .expect("read raw response");
    assert!(
        response.starts_with(b"HTTP/1.1 400"),
        "non-text subprotocol header must be rejected with 400, got {:?}",
        String::from_utf8_lossy(&response)
    );
    server.await.expect("join");
}
