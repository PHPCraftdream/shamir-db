//! Tests for length-prefix msgpack framing.

use shamir_transport_tcp::framing::{
    read_frame, write_close, write_frame, FrameError, MAX_FRAME_SIZE_DEFAULT,
};
use tokio::io::duplex;

#[tokio::test]
async fn round_trip_small_payload() {
    let (mut a, mut b) = duplex(64 * 1024);
    let payload = b"hello world".to_vec();
    write_frame(&mut a, &payload).await.unwrap();
    let got = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
    assert_eq!(got, payload);
}

#[tokio::test]
async fn round_trip_many_frames_in_sequence() {
    let (mut a, mut b) = duplex(64 * 1024);
    let writer = tokio::spawn(async move {
        for i in 0u8..16 {
            let payload = vec![i; (i as usize + 1) * 17];
            write_frame(&mut a, &payload).await.unwrap();
        }
    });

    for i in 0u8..16 {
        let frame = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
        let expected = vec![i; (i as usize + 1) * 17];
        assert_eq!(frame, expected);
    }
    writer.await.unwrap();
}

#[tokio::test]
async fn close_frame_returns_peer_close_error() {
    let (mut a, mut b) = duplex(16);
    write_close(&mut a).await.unwrap();
    let err = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap_err();
    assert!(matches!(err, FrameError::PeerClose));
}

#[tokio::test]
async fn rejects_oversized_frame_declaration() {
    let (mut a, mut b) = duplex(16);
    let too_big = (MAX_FRAME_SIZE_DEFAULT as u32 + 1).to_be_bytes();
    use tokio::io::AsyncWriteExt;
    a.write_all(&too_big).await.unwrap();
    a.flush().await.unwrap();
    let err = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap_err();
    assert!(matches!(err, FrameError::TooLarge { .. }));
}

#[tokio::test]
async fn handles_partial_reads_via_read_exact() {
    let (mut a, mut b) = duplex(8);
    // Tiny duplex buffer forces fragmentation. read_exact must reassemble.
    let writer = tokio::spawn(async move {
        let payload = vec![0xab; 1024];
        write_frame(&mut a, &payload).await.unwrap();
    });
    let frame = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
    assert_eq!(frame.len(), 1024);
    assert!(frame.iter().all(|&b| b == 0xab));
    writer.await.unwrap();
}

#[tokio::test]
async fn frame_exactly_at_size_limit_is_accepted() {
    let (mut a, mut b) = duplex(2 * 1024 * 1024);
    let cap = 1024 * 1024;
    let writer = tokio::spawn(async move {
        let payload = vec![0xcd; cap];
        write_frame(&mut a, &payload).await.unwrap();
    });
    let frame = read_frame(&mut b, cap).await.unwrap();
    assert_eq!(frame.len(), cap);
    writer.await.unwrap();
}
