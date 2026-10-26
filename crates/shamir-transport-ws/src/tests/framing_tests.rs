//! Regression test for ownership-preserving WS prereserved sends.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Sink;
use tokio_tungstenite::tungstenite::Message;

use crate::framing::ws_send_sink_prereserved;

struct PointerSink(Option<*const u8>);

#[tokio::test]
async fn rv_transport_ws_b8ed1366_heartbeat_pong_does_not_consume_control_budget() {
    use futures_util::SinkExt;
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::{protocol::Role, Message};
    use tokio_tungstenite::WebSocketStream;

    let (server_io, client_io) = duplex(64 * 1024);
    let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let state = crate::heartbeat::HeartbeatState::default();
    let (ping, _) = state.next_ping();
    let sender = tokio::spawn(async move {
        for _ in 0..64 {
            client.send(Message::Ping(vec![1].into())).await.unwrap();
        }
        client
            .send(Message::Pong(ping.to_vec().into()))
            .await
            .unwrap();
        client
            .send(Message::Binary(vec![0, 0, 0, 0].into()))
            .await
            .unwrap();
    });
    let result =
        crate::framing::ws_recv_into_stream_observing(&mut server, 1024, &mut Vec::new(), &state)
            .await;
    assert!(
        result.is_ok(),
        "64 controls plus one matched Pong should fit budget: {result:?}"
    );
    sender.await.unwrap();
}

/// Matched heartbeat Pong frames are consumed before flood-budget accounting.
#[test]
fn rv_transport_ws_a3b8161a_matched_pong_correlates_only_latest_ping() {
    let state = crate::heartbeat::HeartbeatState::default();
    let (first, _) = state.next_ping();
    assert!(crate::heartbeat::is_matched_heartbeat_pong(
        Some(&state),
        &first
    ));
    let (second, _) = state.next_ping();
    assert!(!crate::heartbeat::is_matched_heartbeat_pong(
        Some(&state),
        &first
    ));
    assert!(crate::heartbeat::is_matched_heartbeat_pong(
        Some(&state),
        &second
    ));
    assert!(!crate::heartbeat::is_matched_heartbeat_pong(
        Some(&state),
        &second
    ));
    assert!(!crate::heartbeat::is_matched_heartbeat_pong(
        Some(&state),
        &[0; 7]
    ));
}

impl Sink<Message> for PointerSink {
    type Error = tokio_tungstenite::tungstenite::Error;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        if let Message::Binary(bytes) = item {
            self.0 = Some(bytes.as_ptr());
        }
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// RV-TRANSPORT-WS-B7D06174: the sink receives the same backing allocation
/// that the caller owned, proving the WS adapter performs no payload copy.
#[tokio::test]
async fn rv_transport_ws_b7d06174_owned_send_preserves_allocation() {
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(&4u32.to_be_bytes());
    buf.extend_from_slice(b"data");
    let original = buf.as_ptr();
    let mut sink = PointerSink(None);
    ws_send_sink_prereserved(&mut sink, buf).await.unwrap();
    assert_eq!(sink.0, Some(original));
}
