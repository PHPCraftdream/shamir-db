//! Regression tests for card RV-CLIENT-D4D604CA: `read_frame_into` must
//! hand the reader a zero-filled destination (the old `unsafe set_len`
//! exposed stale or uninitialized bytes) and leave the caller's buffer
//! reusable after an error or a cancelled read.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

use crate::framing::{
    read_frame, read_frame_into, write_frame, write_frame_into, write_frame_prereserved,
    FrameError, MAX_FRAME_SIZE_DEFAULT,
};

/// Serves a 4-byte length prefix, then records whether the destination
/// slice it is handed for the payload contains CALLER-owned stale bytes
/// before writing into it. The oracle: the reader must never be handed
/// caller-owned stale bytes (e.g. 0xAA residue from the caller's buffer);
/// residue THIS reader itself wrote in an earlier poll of the same scratch
/// chunk is not caller-owned, so only the region at/after `written` — the
/// part this reader has not filled in previous polls — is inspected.
struct InspectingReader {
    prefix: [u8; 4],
    prefix_sent: bool,
    payload: Vec<u8>,
    saw_dirty_destination: bool,
    /// Bytes this reader itself wrote into the destination in earlier polls.
    written: usize,
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
        // slice, fully marked initialized: inspect only the region this
        // reader has not written in earlier polls — bytes in [0..written)
        // are this reader's own prior output, not caller-owned residue.
        if buf.initialized()[self.written..].iter().any(|&b| b != 0) {
            self.saw_dirty_destination = true;
        }
        let payload = std::mem::take(&mut self.payload);
        buf.put_slice(&payload);
        self.written += payload.len();
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
    // `set_len` exposed exactly these caller-owned stale bytes to the
    // reader. The oracle flags only caller-owned residue, never the
    // reader's own prior writes within the same scratch chunk.
    let mut buf = vec![0xAAu8; 64];
    buf.clear();
    let payload = b"hello".to_vec();
    let mut reader = InspectingReader {
        prefix: (payload.len() as u32).to_be_bytes(),
        prefix_sent: false,
        payload: payload.clone(),
        saw_dirty_destination: false,
        written: 0,
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

/// RV-TRANSPORT-TCP-FF2983EC: oversize rejection consumes the prefix without
/// draining the body; retrying would parse body bytes as another prefix.
#[tokio::test]
async fn rv_transport_tcp_ff2983ec_oversize_error_requires_discarding_stream() {
    let mut wire = (5u32).to_be_bytes().to_vec();
    wire.extend_from_slice(b"abcde");
    wire.extend_from_slice(&(1u32).to_be_bytes());
    wire.push(b'x');
    let mut reader = wire.as_slice();
    assert!(matches!(
        read_frame(&mut reader, 4).await,
        Err(FrameError::TooLarge { .. })
    ));
    assert_eq!(
        reader, b"abcde\0\0\0\x01x",
        "body remains unread, so retry would desynchronize"
    );
}

/// RV-TRANSPORT-TCP-884DDBF5: both invalid input classes are diagnosed as malformed.
#[tokio::test]
async fn rv_transport_tcp_884ddbf5_short_prefix_and_mismatch_are_malformed() {
    let mut writer = tokio::io::sink();
    let short = write_frame_prereserved(&mut writer, &[0, 1, 2]).await;
    assert!(matches!(short, Err(FrameError::MalformedBuffer(_))));
    let mismatch = write_frame_prereserved(&mut writer, &[0, 0, 0, 2, b'x']).await;
    assert!(matches!(mismatch, Err(FrameError::MalformedBuffer(_))));
}

#[test]
fn rv_transport_tcp_a3b66c24_public_reader_contract_documents_cancel_terminally() {
    let source = include_str!("../framing.rs");
    assert!(source.contains("Cancellation also loses the partial frame progress"));
    assert!(source.contains("discard the stream rather than restarting the frame read"));
}

#[tokio::test]
async fn rv_transport_tcp_d1882068_exporter_failure_is_not_a_binding_value() {
    struct Failed;
    impl crate::tls::ConnectionExporter for Failed {
        fn export_keying_material(
            &self,
            _out: &mut [u8],
            _label: &[u8],
            _context: Option<&[u8]>,
        ) -> Result<(), rustls::Error> {
            Err(rustls::Error::General("injected error".into()))
        }
    }
    assert!(crate::tls::extract_tls_exporter(&Failed).is_err());
}

#[test]
fn rv_transport_tcp_884ddbf5_diagnostic_variant_is_distinct() {
    let malformed = FrameError::MalformedBuffer("wrong payload size");
    assert!(malformed.to_string().contains("malformed preframed buffer"));
}

#[tokio::test]
async fn rv_transport_tcp_bbf93e33_fresh_reader_does_not_preallocate_declaration() {
    let (mut writer, mut reader) = tokio::io::duplex(32);
    writer
        .write_all(&(1024u32 * 1024).to_be_bytes())
        .await
        .unwrap();
    writer.write_all(b"x").await.unwrap();
    let mut output = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        crate::framing::read_frame_into(&mut reader, 2 * 1024 * 1024, &mut output),
    )
    .await;
    assert!(
        read.is_err(),
        "the declared body is incomplete; bounded timeout ends the read"
    );
    assert_eq!(output, b"x");
    assert!(
        output.capacity() < 1024 * 1024,
        "capacity follows bytes received rather than declared size"
    );
}

#[tokio::test]
async fn rv_client_d4d604ca_short_read_sees_zeroed_destination_then_errors() {
    // Prefix announces 8 bytes, only 3 arrive before EOF; the caller-owned
    // stale 0xAA capacity must not be presented to the reader on the error
    // path (the reader's own prior writes are not caller-owned residue).
    let mut buf = vec![0xAAu8; 64];
    buf.clear();
    let mut reader = InspectingReader {
        prefix: (8u32).to_be_bytes(),
        prefix_sent: false,
        payload: b"abc".to_vec(),
        saw_dirty_destination: false,
        written: 0,
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
async fn rv_client_d4d604ca_cancelled_partial_read_leaves_received_bytes_only() {
    let (mut tx, mut rx) = tokio::io::duplex(64);
    // Prefix for 10 bytes plus 4 of them: the read parks mid-payload.
    let mut partial = (10u32).to_be_bytes().to_vec();
    partial.extend_from_slice(b"0123");
    tx.write_all(&partial).await.expect("write");
    // Stale capacity is not made live; only bytes received remain in length.
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
    assert_eq!(
        buf, b"0123",
        "only bytes actually received stay live after cancellation"
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
