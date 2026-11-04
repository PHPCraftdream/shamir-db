//! `WsByteStream` over two in-memory `WebSocketStream`s (client and server roles).

use std::io::ErrorKind;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

use crate::client::transport::WsByteStream;

const STEP: Duration = Duration::from_secs(5);

async fn pair() -> (WsByteStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (client_io, server_io) = duplex(64 * 1024);
    let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    (WsByteStream::new(client), server)
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

#[tokio::test]
async fn prefix_and_payload_written_separately_arrive_as_one_message() {
    let (mut client, mut server) = pair().await;
    let expected = frame(b"hello");
    client.write_all(&expected[..4]).await.unwrap();
    client.flush().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.next())
            .await
            .is_err()
    );
    client.write_all(&expected[4..]).await.unwrap();
    client.flush().await.unwrap();
    assert_eq!(next_binary(&mut server).await, expected);
}

#[tokio::test]
async fn prefix_and_payload_in_one_buffer_arrive_as_one_message() {
    let (mut client, mut server) = pair().await;
    let expected = frame(b"hello");
    client.write_all(&expected).await.unwrap();
    client.flush().await.unwrap();
    assert_eq!(next_binary(&mut server).await, expected);
}

#[tokio::test]
async fn two_frames_in_one_write_become_two_messages() {
    let (mut client, mut server) = pair().await;
    let first = frame(b"first");
    let second = frame(b"second");
    let mut both = first.clone();
    both.extend_from_slice(&second);
    client.write_all(&both).await.unwrap();
    client.flush().await.unwrap();
    assert_eq!(next_binary(&mut server).await, first);
    assert_eq!(next_binary(&mut server).await, second);
}

#[tokio::test]
async fn frame_split_across_messages_is_read_contiguously() {
    let (mut client, mut server) = pair().await;
    let full = frame(b"hello world");
    server
        .send(Message::Binary(full[..3].to_vec().into()))
        .await
        .unwrap();
    server
        .send(Message::Binary(full[3..9].to_vec().into()))
        .await
        .unwrap();
    server
        .send(Message::Binary(full[9..].to_vec().into()))
        .await
        .unwrap();
    assert_eq!(read_exactly(&mut client, full.len()).await, full);
}

#[tokio::test]
async fn frame_is_served_across_small_reads() {
    let (mut client, mut server) = pair().await;
    let full = frame(b"hello world");
    server
        .send(Message::Binary(full.clone().into()))
        .await
        .unwrap();
    let mut got = read_exactly(&mut client, 2).await;
    got.extend_from_slice(&read_exactly(&mut client, full.len() - 2).await);
    assert_eq!(got, full);
}

#[tokio::test]
async fn ping_is_skipped_and_data_follows() {
    let (mut client, mut server) = pair().await;
    let full = frame(b"after ping");
    server
        .send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    server
        .send(Message::Binary(full.clone().into()))
        .await
        .unwrap();
    assert_eq!(read_exactly(&mut client, full.len()).await, full);
}

#[tokio::test]
async fn text_message_is_invalid_data() {
    let (mut client, mut server) = pair().await;
    server
        .send(Message::Text("not binary".into()))
        .await
        .unwrap();
    let mut buf = [0u8; 1];
    let err = tokio::time::timeout(STEP, client.read(&mut buf))
        .await
        .expect("read in time")
        .expect_err("text must fail");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[tokio::test]
async fn peer_close_is_end_of_stream() {
    let (mut client, mut server) = pair().await;
    server.send(Message::Close(None)).await.unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(STEP, client.read(&mut buf))
        .await
        .expect("read in time")
        .expect("read ok");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn oversized_declared_length_is_invalid_data() {
    let (mut client, _server) = pair().await;
    let err = client
        .write_all(&u32::MAX.to_be_bytes())
        .await
        .expect_err("oversized must fail");
    assert_eq!(err.kind(), ErrorKind::InvalidData);

    let (mut client, _server) = pair().await;
    let err = client
        .write_all(&((MAX_FRAME_SIZE_DEFAULT + 1) as u32).to_be_bytes())
        .await
        .expect_err("just over the cap must fail");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[tokio::test]
async fn shutdown_sends_a_close_message() {
    let (mut client, mut server) = pair().await;
    client.shutdown().await.unwrap();
    let msg = tokio::time::timeout(STEP, server.next())
        .await
        .expect("message in time")
        .expect("stream open")
        .expect("websocket ok");
    assert!(matches!(msg, Message::Close(_)), "{msg:?}");
}
