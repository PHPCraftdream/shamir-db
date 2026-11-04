//! `WsByteStream` in Raw mode over two in-memory `WebSocketStream`s.

use std::io::ErrorKind;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

use crate::client::transport::{WsByteStream, WsPeerClose};

const STEP: Duration = Duration::from_secs(5);

async fn pair() -> (WsByteStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (client_io, server_io) = duplex(64 * 1024);
    let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    (WsByteStream::raw(client), server)
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

async fn next_binary(server: &mut WebSocketStream<DuplexStream>) -> Vec<u8> {
    let message = tokio::time::timeout(STEP, server.next())
        .await
        .expect("message in time")
        .expect("stream open")
        .expect("websocket ok");
    match message {
        Message::Binary(bytes) => bytes.to_vec(),
        other => panic!("expected BINARY, got {other:?}"),
    }
}

async fn read_exactly(client: &mut WsByteStream<DuplexStream>, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    tokio::time::timeout(STEP, client.read_exact(&mut out))
        .await
        .expect("bytes in time")
        .expect("read ok");
    out
}

async fn read_err(client: &mut WsByteStream<DuplexStream>) -> std::io::Error {
    let mut one = [0u8; 1];
    tokio::time::timeout(STEP, client.read(&mut one))
        .await
        .expect("read in time")
        .expect_err("read must fail")
}

#[tokio::test]
async fn outgoing_frame_arrives_without_prefix() {
    let (mut client, mut server) = pair().await;
    client.write_all(&frame(b"hello")).await.unwrap();
    client.flush().await.unwrap();
    assert_eq!(next_binary(&mut server).await, b"hello".to_vec());
}

#[tokio::test]
async fn split_writes_produce_one_message() {
    let (mut client, mut server) = pair().await;
    let whole = frame(b"hello");
    for byte in &whole[..4] {
        client.write_all(&[*byte]).await.unwrap();
    }
    client.flush().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.next())
            .await
            .is_err()
    );
    client.write_all(&whole[4..]).await.unwrap();
    client.flush().await.unwrap();
    assert_eq!(next_binary(&mut server).await, b"hello".to_vec());
}

#[tokio::test]
async fn incoming_message_gets_prefix() {
    let (mut client, mut server) = pair().await;
    server
        .send(Message::Binary(b"world".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(read_exactly(&mut client, 9).await, frame(b"world"));
}

#[tokio::test]
async fn empty_incoming_message_is_invalid_data() {
    let (mut client, mut server) = pair().await;
    server
        .send(Message::Binary(Vec::new().into()))
        .await
        .unwrap();
    let err = read_err(&mut client).await;
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(
        err.to_string(),
        "raw WebSocket message length outside the frame limit"
    );
}

#[tokio::test]
async fn oversized_incoming_message_is_invalid_data() {
    // tungstenite's own limits are lifted so the Raw length check is the one that fires.
    let lifted = WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None);
    let (client_io, server_io) = duplex(1 << 20);
    let client_ws = WebSocketStream::from_raw_socket(client_io, Role::Client, Some(lifted)).await;
    let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut client = WsByteStream::raw(client_ws);
    let send = async {
        server
            .send(Message::Binary(
                vec![0u8; MAX_FRAME_SIZE_DEFAULT + 1].into(),
            ))
            .await
            .expect("client consumes the whole message");
    };
    let ((), err) = tokio::join!(send, read_err(&mut client));
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(
        err.get_ref()
            .is_some_and(|inner| inner.downcast_ref::<WsError>().is_none()),
        "must be the Raw length check, not a tungstenite error: {err}"
    );
    assert_eq!(
        err.to_string(),
        "raw WebSocket message length outside the frame limit"
    );
}

#[tokio::test]
async fn close_with_code_is_reported_once_then_eof() {
    let (mut client, mut server) = pair().await;
    server
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(4011),
            reason: "token_revoked".into(),
        })))
        .await
        .unwrap();
    let err = read_err(&mut client).await;
    assert_eq!(err.kind(), ErrorKind::ConnectionAborted);
    let close = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<WsPeerClose>())
        .expect("WsPeerClose inside io::Error");
    assert_eq!(close.code, 4011);
    assert_eq!(&*close.reason, "token_revoked");
    let mut one = [0u8; 1];
    let n = tokio::time::timeout(STEP, client.read(&mut one))
        .await
        .expect("read in time")
        .expect("eof is ok");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn close_without_frame_is_plain_eof() {
    let (mut client, mut server) = pair().await;
    server.send(Message::Close(None)).await.unwrap();
    let mut one = [0u8; 1];
    let n = tokio::time::timeout(STEP, client.read(&mut one))
        .await
        .expect("read in time")
        .expect("eof is ok");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn zero_length_outgoing_frame_is_invalid_data() {
    let (mut client, _server) = pair().await;
    let err = client.write_all(&[0, 0, 0, 0]).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn reason_drops_non_printable_and_non_ascii() {
    let close = WsPeerClose::new(4010, "a\u{e9}b\nc\u{7f}d");
    assert_eq!(&*close.reason, "abcd");
}

#[test]
fn reason_is_cut_to_123_bytes() {
    let long = "x".repeat(300);
    let close = WsPeerClose::new(4010, &long);
    assert_eq!(close.reason.len(), 123);
}
