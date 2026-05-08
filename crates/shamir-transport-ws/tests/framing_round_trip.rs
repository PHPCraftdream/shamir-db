//! WS framing round-trip via `tokio-tungstenite` over a duplex pipe.

use shamir_transport_ws::framing::{ws_recv, ws_recv_into, ws_send, MAX_WS_FRAME_SIZE};
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
        assert_eq!(buf.capacity(), initial_cap, "scratch capacity must not grow");
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
        ws_send(&mut client, &vec![0xa5u8; 200]).await.unwrap();
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
