use std::sync::Arc;
use std::time::Duration;

use futures::SinkExt;
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::client::transport::{WsByteStream, WsPeerClose};
use crate::client::LifecycleHooks;
use crate::error::CloseCause;
use crate::{Client, ClientError};

#[test]
fn peer_close_converts_to_peer_closed_cause() {
    assert_eq!(
        WsPeerClose::new(4011, "x").into_cause(),
        CloseCause::PeerClosed {
            code: 4011,
            reason: Arc::from("x"),
        }
    );
}

#[tokio::test]
async fn reader_task_maps_ws_close_to_peer_closed() {
    let (client_io, server_io) = duplex(64 * 1024);
    let client_ws = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let (sink, _sink_peer) = duplex(64 * 1024);
    let client = Client::for_test(
        WsByteStream::raw(client_ws),
        sink,
        LifecycleHooks::default(),
    );
    server
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(4011),
            reason: "token_revoked".into(),
        })))
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), client.ping())
        .await
        .expect("ping resolves once the reader saw the close");
    match result {
        Err(ClientError::ConnectionClosed(CloseCause::PeerClosed { code, reason })) => {
            assert_eq!(code, 4011);
            assert_eq!(&*reason, "token_revoked");
        }
        other => panic!("expected PeerClosed(4011), got {other:?}"),
    }
}
