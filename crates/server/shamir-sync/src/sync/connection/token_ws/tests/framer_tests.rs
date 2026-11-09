use futures_util::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use super::super::framer::{TokenWsFrameReader, TokenWsFrameWriter};
use crate::framer::{CloseIntent, FrameReader, FrameWriter, FramerError};
use crate::sync::connection::request_loop::WriterMsg;
use crate::sync::connection::session::writer_task::run_writer_task;

async fn pair() -> (
    TokenWsFrameReader<tokio::io::DuplexStream>,
    TokenWsFrameWriter<tokio::io::DuplexStream>,
    WebSocketStream<tokio::io::DuplexStream>,
) {
    let (server, client) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let heartbeat = shamir_transport_ws::HeartbeatState::default();
    let (writer, reader) = server.split();
    (
        TokenWsFrameReader::with_heartbeat(reader, heartbeat.clone()),
        TokenWsFrameWriter::with_heartbeat(writer, heartbeat),
        client,
    )
}

#[tokio::test]
async fn raw_writer_strips_shared_loop_prefix_and_reader_reads_payload() {
    let (mut reader, mut writer, mut peer) = pair().await;
    let payload = b"raw msgpack";
    let mut prefixed = (payload.len() as u32).to_be_bytes().to_vec();
    prefixed.extend_from_slice(payload);
    writer
        .write_frame_prereserved(prefixed)
        .await
        .expect("write");
    let frame = peer.next().await.expect("frame").expect("websocket");
    assert!(matches!(frame, Message::Binary(ref bytes) if bytes.as_ref() == payload));
    peer.send(Message::Binary(payload.to_vec().into()))
        .await
        .expect("send");
    let mut received = Vec::new();
    reader
        .read_frame_into(1024, &mut received)
        .await
        .expect("read");
    assert_eq!(received, payload);
}

#[tokio::test]
async fn raw_writer_rejects_wrong_declared_length() {
    let (_reader, mut writer, _peer) = pair().await;
    let error = writer
        .write_frame_prereserved(vec![0, 0, 0, 2, 9])
        .await
        .unwrap_err();
    assert!(matches!(error, FramerError::Decode(_)));
}

#[tokio::test]
async fn raw_reader_rejects_text_frames() {
    let (mut reader, _writer, mut peer) = pair().await;
    peer.send(Message::Text("not binary".into()))
        .await
        .expect("send");
    let mut received = Vec::new();
    let error = reader
        .read_frame_into(1024, &mut received)
        .await
        .unwrap_err();
    assert!(matches!(error, FramerError::UnsupportedData(_)));
    assert_eq!(error.ws_close_code(), Some((1003, "unsupported data")));
}

#[tokio::test]
async fn raw_reader_rejects_oversized_binary_message() {
    let (mut reader, _writer, mut peer) = pair().await;
    let payload = vec![0_u8; 101];
    let send = peer.send(Message::Binary(payload.into()));
    let mut received = Vec::new();
    let read = reader.read_frame_into(100, &mut received);
    let (sent, result) = tokio::join!(send, read);
    sent.expect("bounded message send");
    assert!(matches!(
        result,
        Err(FramerError::TooLarge {
            actual: 101,
            max: 100
        })
    ));
    assert_eq!(
        result
            .as_ref()
            .unwrap_err()
            .ws_close_code()
            .map(|(code, _)| code),
        Some(1009)
    );
}

#[tokio::test(start_paused = true)]
async fn writer_heartbeat_timeout_closes_with_4003_without_matching_pong() {
    let (server_io, peer_io) = duplex(64 * 1024);
    let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let mut peer = WebSocketStream::from_raw_socket(peer_io, Role::Client, None).await;
    let (sink, _reader) = server.split();
    let heartbeat = shamir_transport_ws::HeartbeatState::default();
    let mut writer = TokenWsFrameWriter::with_heartbeat(sink, heartbeat.clone());
    let (sender, mut receiver) = mpsc::channel::<WriterMsg>(1);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _started = started_tx.send(());
        run_writer_task(
            &mut writer,
            &mut receiver,
            Some(heartbeat),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            CloseIntent::new(),
        )
        .await;
    });

    started_rx.await.expect("writer task started");
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let mut peer_read = Box::pin(peer.next());
    let ping = tokio::time::timeout(std::time::Duration::from_secs(5), &mut peer_read)
        .await
        .expect("bounded ping read")
        .expect("ping frame")
        .expect("valid ping");
    assert!(matches!(ping, Message::Ping(_)));
    drop(peer_read);
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("bounded writer join")
        .expect("writer task");
    let close = tokio::time::timeout(std::time::Duration::from_secs(5), peer.next())
        .await
        .expect("bounded close read")
        .expect("close frame")
        .expect("valid close");
    assert!(matches!(close, Message::Close(Some(frame)) if u16::from(frame.code) == 4003));
    drop(sender);
}
