//! Regression tests for card RV-CLIENT-D4D604CA: `read_frame_into` must
//! hand the reader a zero-filled destination (the old `unsafe set_len`
//! exposed stale or uninitialized bytes) and leave the caller's buffer
//! reusable after an error or a cancelled read.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

use crate::framing::{
    read_frame_into, write_frame, write_frame_into, write_frame_prereserved, FrameError,
    MAX_FRAME_SIZE_DEFAULT,
};

/// Serves a 4-byte length prefix, then records whether the destination
/// slice it is handed for the payload is all zeroes before writing into it.
struct InspectingReader {
    prefix: [u8; 4],
    prefix_sent: bool,
    payload: Vec<u8>,
    saw_dirty_destination: bool,
}

impl AsyncRead for InspectingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.prefix_sent {
            let prefix = self.prefix;
            buf.put_slice(&prefix);
            self.prefix_sent = true;
            return Poll::Ready(Ok(()));
        }
        // The payload destination is a `ReadBuf` over the caller's
        // slice, fully marked initialized: inspect it before filling.
        if buf.initialized()[buf.filled().len()..]
            .iter()
            .any(|&b| b != 0)
        {
            self.saw_dirty_destination = true;
        }
        let payload = std::mem::take(&mut self.payload);
        buf.put_slice(&payload);
        Poll::Ready(Ok(()))
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

#[tokio::test]
async fn rv_client_d4d604ca_reader_gets_zeroed_destination() {
    // Previously initialized capacity holding 0xAA: the old `clear` +
    // `set_len` exposed exactly these stale bytes to the reader.
    let mut buf = vec![0xAAu8; 64];
    buf.clear();
    let payload = b"hello".to_vec();
    let mut reader = InspectingReader {
        prefix: (payload.len() as u32).to_be_bytes(),
        prefix_sent: false,
        payload: payload.clone(),
        saw_dirty_destination: false,
    };
    read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .expect("frame");
    assert!(
        !reader.saw_dirty_destination,
        "reader was handed stale bytes as an initialized destination"
    );
    assert_eq!(buf, payload);
}

#[tokio::test]
async fn rv_client_d4d604ca_short_read_sees_zeroed_destination_then_errors() {
    // Prefix announces 8 bytes, only 3 arrive before EOF; the stale 0xAA
    // capacity must not be presented to the reader on the error path.
    let mut buf = vec![0xAAu8; 64];
    buf.clear();
    let mut reader = InspectingReader {
        prefix: (8u32).to_be_bytes(),
        prefix_sent: false,
        payload: b"abc".to_vec(),
        saw_dirty_destination: false,
    };
    let err = read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .expect_err("short read must fail");
    match err {
        FrameError::Io(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(
        !reader.saw_dirty_destination,
        "reader was handed stale bytes as an initialized destination"
    );
    assert!(buf.is_empty(), "failed read must leave an empty buffer");
    let next = frame(b"next-frame");
    read_frame_into(&mut next.as_slice(), MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .expect("reuse after error");
    assert_eq!(buf, b"next-frame");
}

#[tokio::test]
async fn rv_client_d4d604ca_cancelled_partial_read_leaves_only_peer_bytes_and_zeroes() {
    let (mut tx, mut rx) = tokio::io::duplex(64);
    // Prefix for 10 bytes plus 4 of them: the read parks mid-payload.
    let mut partial = (10u32).to_be_bytes().to_vec();
    partial.extend_from_slice(b"0123");
    tx.write_all(&partial).await.expect("write");
    // Stale 0xAA capacity: the old `set_len` left it in the live length.
    let mut buf = vec![0xAAu8; 64];
    buf.clear();
    // `biased` polls the read first (it parks), then the ready branch
    // wins and the read future is dropped: a deterministic cancellation.
    tokio::select! {
        biased;
        r = read_frame_into(&mut rx, MAX_FRAME_SIZE_DEFAULT, &mut buf) => {
            panic!("read must not complete: {r:?}");
        }
        _ = std::future::ready(()) => {}
    }
    let mut expected = b"0123".to_vec();
    expected.resize(10, 0);
    assert_eq!(
        buf, expected,
        "live length holds only peer bytes and zero-fill"
    );
    let next = frame(b"fresh");
    read_frame_into(&mut next.as_slice(), MAX_FRAME_SIZE_DEFAULT, &mut buf)
        .await
        .expect("reuse after cancel");
    assert_eq!(buf, b"fresh");
}

/// Regression test for card RV-TRANSPORT-TCP-3A576732: a zero-length payload
/// passed to the framing writers used to silently emit a 4-byte zero length
/// on the wire, which readers interpret as the graceful-close sentinel. All
/// writers must reject empty payloads with [`FrameError::EmptyPayload`]
/// instead of encoding them.
#[tokio::test]
async fn rv_transport_tcp_3a576732_empty_write_rejected_not_close_sentinel() {
    let (mut tx, mut rx) = tokio::io::duplex(64);
    let err = write_frame(&mut tx, b"").await.expect_err("write_frame");
    match err {
        FrameError::EmptyPayload => {}
        other => panic!("unexpected error: {other:?}"),
    }

    let mut scratch = Vec::with_capacity(64);
    let err = write_frame_into(&mut tx, b"", &mut scratch)
        .await
        .expect_err("write_frame_into");
    match err {
        FrameError::EmptyPayload => {}
        other => panic!("unexpected error: {other:?}"),
    }

    let err = write_frame_prereserved(&mut tx, &[0, 0, 0, 0])
        .await
        .expect_err("write_frame_prereserved");
    match err {
        FrameError::EmptyPayload => {}
        other => panic!("unexpected error: {other:?}"),
    }

    // Nothing may have been written to the wire: the read side must see EOF
    // with zero bytes (dropping `tx` closes it).
    drop(tx);
    let mut sink = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut rx, &mut sink)
        .await
        .expect("read_to_end");
    assert!(
        sink.is_empty(),
        "writers emitted bytes for an empty payload"
    );
}
