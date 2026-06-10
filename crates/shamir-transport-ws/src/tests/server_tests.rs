use futures_util::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use crate::framing::MAX_WS_FRAME_SIZE;
use crate::server::{accept_native_ws, server_ws_config};

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
        let mut ws = accept_native_ws(server_io).await.expect("accept");
        // Reading the oversized message must error (Capacity), never
        // yield a 16 MiB+ buffered payload.
        match ws.next().await {
            Some(Err(e)) => format!("err: {e}"),
            Some(Ok(m)) => panic!("expected error, got message len {}", m.len()),
            None => "closed".to_string(),
        }
    });

    // Client side: uncapped so it emits the oversized frame verbatim.
    let client_cfg = WebSocketConfig {
        max_message_size: None,
        max_frame_size: None,
        ..Default::default()
    };
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
    let _ = client.send(Message::Binary(oversized)).await;

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
        let mut ws = accept_native_ws(server_io).await.expect("accept");
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
        .send(Message::Binary(b"hello".to_vec()))
        .await
        .expect("send");

    let got = server.await.expect("join");
    assert_eq!(got, b"hello");
}
