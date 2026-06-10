//! Unit-level tests for `Framer::split` — proves that after splitting a
//! framer the reader and writer halves work independently, and that both
//! halves can be driven from separate tasks concurrently.
//!
//! Uses `tokio::io::duplex` as the underlying transport for `TcpFramer`
//! and `tokio_tungstenite::WebSocketStream::from_raw_socket` for
//! `WsFramer`, mirroring the patterns from
//! `shamir-transport-ws/tests/framing_round_trip.rs`.

use shamir_server::framer::{FrameReader, FrameWriter, Framer, TcpFramer, WsFramer};
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

// ---------------------------------------------------------------------------
// TcpFramer split tests
// ---------------------------------------------------------------------------

/// After split, the writer half can send a frame that the reader half
/// receives — sequential, same task.
#[tokio::test]
async fn tcp_split_sequential_round_trip() {
    let (a, b) = duplex(64 * 1024);
    // a = server side, b = client side
    let server_framer = TcpFramer::new(a);
    let client_framer = TcpFramer::new(b);

    let (mut server_reader, mut server_writer) = server_framer.split();
    let (mut client_reader, mut client_writer) = client_framer.split();

    let payload = b"tcp framer split round trip".to_vec();
    let p2 = payload.clone();

    // Client writer → server reader.
    let mut scratch = Vec::new();
    client_writer
        .write_frame_into(&p2, &mut scratch)
        .await
        .unwrap();

    let mut buf = Vec::new();
    server_reader
        .read_frame_into(usize::MAX, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, payload);

    // Server writer → client reader.
    let reply = b"pong".to_vec();
    let r2 = reply.clone();
    server_writer
        .write_frame_into(&r2, &mut scratch)
        .await
        .unwrap();

    client_reader
        .read_frame_into(usize::MAX, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, reply);
}

/// After split, reader and writer can be driven from two separate tasks
/// concurrently — the core duplex-readiness proof for TCP.
#[tokio::test]
async fn tcp_split_concurrent_tasks() {
    let (a, b) = duplex(64 * 1024);
    let server_framer = TcpFramer::new(a);
    let client_framer = TcpFramer::new(b);

    let (mut server_reader, _server_writer) = server_framer.split();
    let (_client_reader, mut client_writer) = client_framer.split();

    let payload = vec![0xabu8; 1024];
    let p2 = payload.clone();

    // Spawn writer task.
    let write_task = tokio::spawn(async move {
        let mut scratch = Vec::new();
        client_writer
            .write_frame_into(&p2, &mut scratch)
            .await
            .unwrap();
    });

    // Reader task (current task).
    let mut buf = Vec::new();
    server_reader
        .read_frame_into(usize::MAX, &mut buf)
        .await
        .unwrap();

    write_task.await.unwrap();
    assert_eq!(buf, payload);
}

/// `FrameWriter::shutdown` on the TCP writer half doesn't panic and the
/// reader sees EOF on the next read attempt.
#[tokio::test]
async fn tcp_split_shutdown_signals_eof() {
    let (a, b) = duplex(64 * 1024);
    let server_framer = TcpFramer::new(a);
    let mut client_framer = TcpFramer::new(b);

    let (_server_reader, mut server_writer) = server_framer.split();

    // Shutdown the server writer half.
    server_writer.shutdown().await;

    // Client tries to read — should get an error (peer closed / IO error).
    let mut buf = Vec::new();
    let result = client_framer.read_frame_into(usize::MAX, &mut buf).await;
    assert!(
        result.is_err(),
        "expected error after remote shutdown, got Ok"
    );
}

// ---------------------------------------------------------------------------
// WsFramer split tests
// ---------------------------------------------------------------------------

/// After split, writer half sends a frame that the reader half receives —
/// sequential, same task.
#[tokio::test]
async fn ws_split_sequential_round_trip() {
    let (a, b) = duplex(64 * 1024);
    let server_ws = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let client_ws = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let server_framer = WsFramer::new(server_ws);
    let client_framer = WsFramer::new(client_ws);

    let (mut server_reader, mut server_writer) = server_framer.split();
    let (mut client_reader, mut client_writer) = client_framer.split();

    let payload = b"ws framer split round trip".to_vec();
    let p2 = payload.clone();

    let mut scratch = Vec::new();
    client_writer
        .write_frame_into(&p2, &mut scratch)
        .await
        .unwrap();

    let mut buf = Vec::new();
    server_reader
        .read_frame_into(usize::MAX, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, payload);

    // Echo back.
    let reply = b"ws pong".to_vec();
    let r2 = reply.clone();
    server_writer
        .write_frame_into(&r2, &mut scratch)
        .await
        .unwrap();

    client_reader
        .read_frame_into(usize::MAX, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, reply);
}

/// WS split halves driven from separate tasks concurrently.
#[tokio::test]
async fn ws_split_concurrent_tasks() {
    let (a, b) = duplex(64 * 1024);
    let server_ws = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let client_ws = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let (mut server_reader, _server_writer) = WsFramer::new(server_ws).split();
    let (_client_reader, mut client_writer) = WsFramer::new(client_ws).split();

    let payload = vec![0xddu8; 512];
    let p2 = payload.clone();

    let write_task = tokio::spawn(async move {
        let mut scratch = Vec::new();
        client_writer
            .write_frame_into(&p2, &mut scratch)
            .await
            .unwrap();
    });

    let read_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        server_reader
            .read_frame_into(usize::MAX, &mut buf)
            .await
            .unwrap();
        buf
    });

    let (write_result, read_result) = tokio::join!(write_task, read_task);
    write_result.unwrap();
    let got = read_result.unwrap();
    assert_eq!(got, payload);
}
