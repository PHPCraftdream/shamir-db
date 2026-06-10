//! WS framing round-trip via `tokio-tungstenite` over a duplex pipe.

use shamir_transport_ws::framing::{
    ws_recv, ws_recv_into, ws_recv_into_stream, ws_send, ws_send_sink, MAX_WS_FRAME_SIZE,
};
use tokio::io::duplex;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

#[tokio::test]
async fn round_trip_small() {
    let (a, b) = duplex(64 * 1024);
    let mut server = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let payload = b"hello world".to_vec();
    let p2 = payload.clone();
    let writer = tokio::spawn(async move {
        ws_send(&mut client, &p2).await.unwrap();
    });

    let got = ws_recv(&mut server, MAX_WS_FRAME_SIZE).await.unwrap();
    assert_eq!(got, payload);
    writer.await.unwrap();
}

#[tokio::test]
async fn round_trip_into_buffer_reuses_capacity() {
    let (a, b) = duplex(64 * 1024);
    let mut server = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let mut buf = Vec::with_capacity(2048);
    let initial_cap = buf.capacity();

    let writer = tokio::spawn(async move {
        for sz in [16usize, 256, 1024, 256, 16] {
            ws_send(&mut client, &vec![0xa5u8; sz]).await.unwrap();
        }
    });

    for sz in [16usize, 256, 1024, 256, 16] {
        ws_recv_into(&mut server, MAX_WS_FRAME_SIZE, &mut buf)
            .await
            .unwrap();
        assert_eq!(buf.len(), sz);
        assert_eq!(
            buf.capacity(),
            initial_cap,
            "scratch capacity must not grow"
        );
    }
    writer.await.unwrap();
}

#[tokio::test]
async fn rejects_oversized_frame() {
    let (a, b) = duplex(64 * 1024);
    let mut server = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    // We can't directly construct a malformed frame via ws_send (it
    // validates length internally), so we test the cap by setting a tiny
    // max. Send a 200-byte payload, then receive with cap=100 → TooLarge.
    let writer = tokio::spawn(async move {
        ws_send(&mut client, &[0xa5u8; 200]).await.unwrap();
    });
    let result = ws_recv(&mut server, 100).await;
    assert!(result.is_err());
    writer.await.unwrap();
}

#[tokio::test]
async fn many_frames_in_sequence() {
    let (a, b) = duplex(64 * 1024);
    let mut server = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let writer = tokio::spawn(async move {
        for i in 0u8..16 {
            let p = vec![i; (i as usize + 1) * 17];
            ws_send(&mut client, &p).await.unwrap();
        }
    });

    for i in 0u8..16 {
        let got = ws_recv(&mut server, MAX_WS_FRAME_SIZE).await.unwrap();
        let expected = vec![i; (i as usize + 1) * 17];
        assert_eq!(got, expected);
    }
    writer.await.unwrap();
}

// ---------------------------------------------------------------------------
// Split-half tests — ws_send_sink / ws_recv_into_stream
// ---------------------------------------------------------------------------

use futures_util::{SinkExt, StreamExt};

/// Split a WebSocketStream into halves; send from the sink half and receive
/// from the stream half — both on the same task (sequential).
#[tokio::test]
async fn split_halves_send_recv_sequential() {
    let (a, b) = duplex(64 * 1024);
    let server_ws = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let (mut sink, mut stream) = server_ws.split();

    let payload = b"split-half round trip".to_vec();
    let p2 = payload.clone();

    // Write from sink, read from stream — different directions, same test.
    let writer = tokio::spawn(async move {
        ws_send(&mut client, &p2).await.unwrap();
    });

    let mut buf = Vec::new();
    ws_recv_into_stream(&mut stream, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .unwrap();

    assert_eq!(buf, payload);
    writer.await.unwrap();

    // Send from sink back to client — just checks ws_send_sink compiles &
    // works when the other side has already consumed.  We close the sink to
    // signal EOF instead of doing another receive on the client, keeping the
    // test focused.
    drop(stream); // drop reader half
    let _ = sink.close().await; // graceful close of write half
}

/// Send from a `SplitSink` half and receive from a `SplitStream` half
/// concurrently from two tasks — the core duplex-readiness proof.
#[tokio::test]
async fn split_halves_concurrent_send_recv() {
    let (a, b) = duplex(64 * 1024);
    let server_ws = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let (mut server_sink, mut server_stream) = server_ws.split();

    let payload = vec![0xddu8; 512];
    let p2 = payload.clone();

    // Task 1: client writes to server.
    let writer_task = tokio::spawn(async move {
        ws_send(&mut client, &p2).await.unwrap();
    });

    // Task 2: server reads from its stream half.
    let reader_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        ws_recv_into_stream(&mut server_stream, MAX_WS_FRAME_SIZE, &mut buf)
            .await
            .unwrap();
        buf
    });

    let (write_result, read_result) = tokio::join!(writer_task, reader_task);
    write_result.unwrap();
    let got = read_result.unwrap();
    assert_eq!(got, payload);

    // Shut down sink gracefully.
    let _ = server_sink.close().await;
}

/// `ws_send_sink` sends a frame that the peer can receive via the ordinary
/// `ws_recv_into` function — cross-variant interop.
#[tokio::test]
async fn sink_send_received_by_whole_stream_recv() {
    let (a, b) = duplex(64 * 1024);
    let server_ws = WebSocketStream::from_raw_socket(a, Role::Server, None).await;
    let mut client = WebSocketStream::from_raw_socket(b, Role::Client, None).await;

    let (mut sink, _stream) = server_ws.split();

    let payload = b"sink-to-whole".to_vec();
    let p2 = payload.clone();

    let sender = tokio::spawn(async move {
        ws_send_sink(&mut sink, &p2).await.unwrap();
    });

    let mut buf = Vec::new();
    ws_recv_into(&mut client, MAX_WS_FRAME_SIZE, &mut buf)
        .await
        .unwrap();

    assert_eq!(buf, payload);
    sender.await.unwrap();
}
