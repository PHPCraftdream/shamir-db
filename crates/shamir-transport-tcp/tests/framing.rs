//! Tests for length-prefix msgpack framing.

use shamir_transport_tcp::framing::{
    read_frame, read_frame_into, write_close, write_frame, write_frame_into, FrameError,
    MAX_FRAME_SIZE_DEFAULT,
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
    let err = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT)
        .await
        .unwrap_err();
    assert!(matches!(err, FrameError::PeerClose));
}

#[tokio::test]
async fn rejects_oversized_frame_declaration() {
    let (mut a, mut b) = duplex(16);
    let too_big = (MAX_FRAME_SIZE_DEFAULT as u32 + 1).to_be_bytes();
    use tokio::io::AsyncWriteExt;
    a.write_all(&too_big).await.unwrap();
    a.flush().await.unwrap();
    let err = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT)
        .await
        .unwrap_err();
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

/// Optim #1 (pooled buffer API): `read_frame_into` reads correctly into a
/// caller-supplied buffer.
#[tokio::test]
async fn read_frame_into_round_trip_small() {
    let (mut a, mut b) = duplex(64 * 1024);
    let payload = b"hello world".to_vec();
    write_frame(&mut a, &payload).await.unwrap();
    let mut buf = Vec::new();
    read_frame_into(&mut b, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf, payload);
}

/// Optim #1: capacity is reused across calls — no growth after first read
/// when subsequent frames fit in existing capacity.
#[tokio::test]
async fn read_frame_into_reuses_capacity() {
    let (mut a, mut b) = duplex(64 * 1024);
    let mut buf = Vec::with_capacity(2048);
    let initial_cap = buf.capacity();

    // 5 frames of decreasing size — buf capacity must NOT change.
    for sz in [1024usize, 512, 256, 128, 64] {
        let payload = vec![0xefu8; sz];
        write_frame(&mut a, &payload).await.unwrap();
        read_frame_into(&mut b, MAX_FRAME_SIZE_DEFAULT, &mut buf)
            .await
            .unwrap();
        assert_eq!(buf.len(), sz);
        assert_eq!(buf.capacity(), initial_cap, "capacity must not shrink");
    }
}

/// Optim #1: handles partial reads via `read_exact` correctly even into
/// pre-set-len buffer (no UB observable).
#[tokio::test]
async fn read_frame_into_handles_partial_reads() {
    let (mut a, mut b) = duplex(8); // tiny buffer forces fragmentation
    let writer = tokio::spawn(async move {
        let payload = vec![0xa5u8; 1024];
        write_frame(&mut a, &payload).await.unwrap();
    });
    let mut buf = Vec::with_capacity(64);
    read_frame_into(&mut b, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .unwrap();
    assert_eq!(buf.len(), 1024);
    assert!(buf.iter().all(|&b| b == 0xa5));
    writer.await.unwrap();
}

/// Optim #1: peer-close on length=0 leaves buffer untouched.
#[tokio::test]
async fn read_frame_into_close_returns_peer_close() {
    let (mut a, mut b) = duplex(16);
    write_close(&mut a).await.unwrap();
    let mut buf = vec![0xffu8; 100];
    let err = read_frame_into(&mut b, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .unwrap_err();
    assert!(matches!(err, FrameError::PeerClose));
}

/// Optim #1: oversized declaration is rejected before any unsafe set_len.
#[tokio::test]
async fn read_frame_into_rejects_oversized() {
    let (mut a, mut b) = duplex(16);
    let too_big = (MAX_FRAME_SIZE_DEFAULT as u32 + 1).to_be_bytes();
    use tokio::io::AsyncWriteExt;
    a.write_all(&too_big).await.unwrap();
    a.flush().await.unwrap();
    let mut buf = Vec::new();
    let err = read_frame_into(&mut b, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .unwrap_err();
    assert!(matches!(err, FrameError::TooLarge { .. }));
    assert_eq!(buf.len(), 0);
}

/// Optim #7: write_frame_into reuses caller buffer + emits len+payload
/// in a single write_all (bench-confirmed halves TLS record overhead).
#[tokio::test]
async fn write_frame_into_round_trip_reuses_buffer() {
    let (mut a, mut b) = duplex(64 * 1024);
    let mut scratch = Vec::with_capacity(2048);
    let initial_cap = scratch.capacity();

    for sz in [16usize, 256, 1024, 256, 16] {
        let payload = vec![0xa5u8; sz];
        write_frame_into(&mut a, &payload, &mut scratch)
            .await
            .unwrap();
        let got = read_frame(&mut b, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
        assert_eq!(got, payload);
    }
    assert_eq!(
        scratch.capacity(),
        initial_cap,
        "scratch capacity must not grow when frames stay below initial size"
    );
}

// ── Miri-safe tests ──────────────────────────────────────────────────────────
//
// These two tests use `std::io::Cursor` (always `Poll::Ready`, no mio/IOCP)
// and a current-thread Tokio runtime built WITHOUT `.enable_io()`.  That means
// the mio `Selector` is never created, so `cargo miri test` can run them.
//
// Run with:
//   cargo +nightly miri test -p shamir-transport-tcp read_frame_into_miri
//
// They vet the `unsafe set_len` path in `read_frame_into`:
//   • happy path  — fully-initialized bytes reach the caller.
//   • error path  — truncated payload → `UnexpectedEof` → buf left empty.

/// Happy-path: bytes written through `unsafe set_len` are provably initialized.
#[test]
fn read_frame_into_miri_roundtrip_no_io_driver() {
    // Current-thread runtime WITHOUT .enable_io() — mio Selector never created.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let payload: &[u8] = b"hello shamir";
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        framed.extend_from_slice(payload);

        let mut cursor = std::io::Cursor::new(framed);
        let mut buf = Vec::new();
        read_frame_into(&mut cursor, 1024, &mut buf).await.unwrap();

        // Proves every byte in the set_len region was overwritten.
        assert_eq!(&buf[..], payload);
    });
}

/// Error-path: truncated payload triggers `UnexpectedEof`; buf must be empty.
#[test]
fn read_frame_into_miri_partial_read_clears_buf() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        // Claim 32 bytes but only supply 4 — `read_exact` will hit EOF.
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(&32u32.to_be_bytes());
        framed.extend_from_slice(&[0xffu8; 4]); // only 4 bytes instead of 32

        let mut cursor = std::io::Cursor::new(framed);
        let mut buf = Vec::new();
        let result = read_frame_into(&mut cursor, 1024, &mut buf).await;

        assert!(
            result.is_err(),
            "expected an error due to truncated payload"
        );
        assert!(
            buf.is_empty(),
            "buf must be cleared after error; found {} bytes",
            buf.len()
        );
    });
}

/// Optim #7: write_frame produces byte-identical wire output to
/// write_frame_into (single concatenated write vs two separate writes
/// from the original implementation).
#[tokio::test]
async fn write_frame_and_write_frame_into_produce_identical_bytes() {
    let payload = vec![0xefu8; 1234];

    let (mut a1, mut b1) = duplex(8 * 1024);
    write_frame(&mut a1, &payload).await.unwrap();
    let frame1 = read_frame(&mut b1, MAX_FRAME_SIZE_DEFAULT).await.unwrap();

    let (mut a2, mut b2) = duplex(8 * 1024);
    let mut scratch = Vec::new();
    write_frame_into(&mut a2, &payload, &mut scratch)
        .await
        .unwrap();
    let frame2 = read_frame(&mut b2, MAX_FRAME_SIZE_DEFAULT).await.unwrap();

    assert_eq!(frame1, frame2);
    assert_eq!(frame1, payload);
}
