use futures_util::StreamExt;
use tokio::io::duplex;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;

use crate::framer::FrameWriter;
use crate::http_ws::cancel_registry::TokenWsCancelRegistry;
use crate::sync::connection::token_ws::{TokenWsCloseGuard, TokenWsFrameWriter};
use shamir_connect::server::token::TokenId;

#[tokio::test]
async fn writer_close_waits_for_registration_cleanup() {
    let registry = std::sync::Arc::new(TokenWsCancelRegistry::default());
    let id = TokenId([12; 16]);
    let registration = registry.register(id);
    let (server_io, peer_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut peer = WebSocketStream::from_raw_socket(peer_io, Role::Client, None).await;
    let (sink, _reader) = server.split();
    let heartbeat = shamir_transport_ws::HeartbeatState::default();
    let writer = TokenWsFrameWriter::with_heartbeat(sink, heartbeat);
    let close_requested = CancellationToken::new();
    let cleanup_complete = CancellationToken::new();
    let mut writer =
        TokenWsCloseGuard::new(writer, close_requested.clone(), cleanup_complete.clone());
    let (closed_tx, closed_rx) = oneshot::channel();
    let close = tokio::spawn(async move {
        writer.close_with(4003, "heartbeat_timeout").await;
        let _ = closed_tx.send(());
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        close_requested.cancelled(),
    )
    .await
    .expect("close request bounded");
    assert!(registry.contains_entry(&id));
    let before_cleanup =
        tokio::time::timeout(std::time::Duration::from_millis(10), peer.next()).await;
    assert!(before_cleanup.is_err(), "close frame sent before cleanup");

    drop(registration);
    assert!(!registry.contains_entry(&id));
    cleanup_complete.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), closed_rx)
        .await
        .expect("close completion bounded")
        .expect("writer close completed");
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), peer.next())
        .await
        .expect("close frame bounded")
        .expect("close frame")
        .expect("valid websocket frame");
    assert!(matches!(frame, Message::Close(Some(frame)) if u16::from(frame.code) == 4003));
    tokio::time::timeout(std::time::Duration::from_secs(1), close)
        .await
        .expect("close task bounded")
        .expect("close task");
}
