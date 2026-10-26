//! WS close-code propagation and prereserved send tests
//! (RV-TRANSPORT-WS-B0F1151C + RV-TRANSPORT-WS-B7D06174).

use futures_util::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::sync::framer::{close_reason, FrameWriter, Framer, FramerError, WsFramer};

async fn ws_pair() -> (
    WsFramer<tokio::io::DuplexStream>,
    WebSocketStream<tokio::io::DuplexStream>,
) {
    let (a, b) = duplex(64 * 1024);
    (
        WsFramer::new(WebSocketStream::from_raw_socket(a, Role::Server, None).await),
        WebSocketStream::from_raw_socket(b, Role::Client, None).await,
    )
}

/// RV-TRANSPORT-WS-B0F1151C: Close carries the exact private code and reason.
#[tokio::test]
async fn rv_transport_ws_b0f1151c_close_with_sends_specified_close_code() {
    let (framer, mut peer) = ws_pair().await;
    let (_, mut writer) = framer.split();
    writer.close_with(4002, "server_busy").await;
    match peer.next().await.expect("close") {
        Ok(Message::Close(Some(frame))) => {
            assert_eq!(u16::from(frame.code), 4002);
            assert_eq!(frame.reason.as_str(), "server_busy");
        }
        other => panic!("expected coded Close, got {other:?}"),
    }
}

/// RV-TRANSPORT-WS-B0F1151C: Text and malformed-length errors retain §6 codes.
#[tokio::test]
async fn rv_transport_ws_b0f1151c_read_errors_carry_close_codes() {
    let (mut framer, mut peer) = ws_pair().await;
    peer.send(Message::Text("nope".into())).await.unwrap();
    let mut buf = Vec::new();
    let err = framer.read_frame_into(1024, &mut buf).await.unwrap_err();
    assert!(matches!(err, FramerError::UnsupportedData(_)));
    assert_eq!(err.ws_close_code(), Some((1003, "unsupported data")));

    let (mut framer, mut peer) = ws_pair().await;
    peer.send(Message::Binary(vec![0, 0, 0, 9, 1, 2].into()))
        .await
        .unwrap();
    let err = framer.read_frame_into(1024, &mut buf).await.unwrap_err();
    assert_eq!(err.ws_close_code(), Some((1002, "protocol error")));
}

/// RV-TRANSPORT-WS-B0F1151C: ordinary shutdown is explicit Close 1000.
#[tokio::test]
async fn rv_transport_ws_b0f1151c_shutdown_sends_close_1000() {
    let (framer, mut peer) = ws_pair().await;
    let (_, mut writer) = framer.split();
    writer.shutdown().await;
    match peer.next().await.expect("close") {
        Ok(Message::Close(Some(frame))) => {
            assert_eq!(u16::from(frame.code), 1000);
            assert_eq!(frame.reason.as_str(), close_reason(1000));
        }
        other => panic!("expected Close 1000, got {other:?}"),
    }
}

/// RV-TRANSPORT-WS-B7D06174: owned prereserved data is sent without rebuilding.
#[tokio::test]
async fn rv_transport_ws_b7d06174_prereserved_writer_sends_buffer_verbatim() {
    let (framer, mut peer) = ws_pair().await;
    let (_, mut writer) = framer.split();
    let payload = b"hello prereserved".to_vec();
    let mut buf = (payload.len() as u32).to_be_bytes().to_vec();
    buf.extend_from_slice(&payload);
    writer.write_frame_prereserved(buf).await.unwrap();
    assert_eq!(
        crate::sync::framer::FramerError::Decode("old".into()).ws_close_code(),
        Some((1002, "protocol error"))
    );
    match peer.next().await.expect("frame") {
        Ok(Message::Binary(bytes)) => {
            assert_eq!(
                u32::from_be_bytes(bytes[..4].try_into().unwrap()),
                payload.len() as u32
            );
            assert_eq!(&bytes[4..], &payload);
        }
        other => panic!("expected BINARY, got {other:?}"),
    }
}
