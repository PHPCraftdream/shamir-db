use futures_util::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use crate::browser::BrowserOriginPolicy;
use crate::framing::MAX_WS_FRAME_SIZE;
use crate::server::{
    accept_browser_ws, accept_native_ws, server_ws_config, WsAcceptError, NATIVE_WS_PATH,
};
use tokio_tungstenite::tungstenite::handshake::client::{generate_key, Request};
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
        "ws://localhost/shamir/v1",
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
        tokio_tungstenite::client_async("ws://localhost/shamir/v1", client_io)
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
fn ws_request(path: &str, origin: Option<&str>) -> Request {
    ws_request_header(
        path,
        "Origin",
        origin.map(|o| http::HeaderValue::from_str(o).expect("valid origin header")),
    )
}

/// Build a WS upgrade request with an explicit path and one raw header
/// (pass `None` to omit it — needed for non-`&str` header values such as
/// obs-text Origin bytes).
fn ws_request_header(path: &str, name: &str, value: Option<http::HeaderValue>) -> Request {
    let mut builder = Request::builder()
        .uri(format!("ws://localhost{path}"))
        .header("Host", "localhost")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key());
    if let Some(v) = value {
        builder = builder.header(name, v);
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
