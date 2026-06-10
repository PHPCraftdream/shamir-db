//! Unit tests for the rid-demux reader logic.
//!
//! We drive `reader_task` directly via `tokio::io::duplex`, injecting
//! hand-crafted response frames and asserting that each waiter receives
//! exactly the payload it expects, regardless of the order frames arrive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use shamir_connect::common::envelope::{ErrorEnvelope, ResponseEnvelope};
use shamir_transport_tcp::framing::write_frame;

use crate::client::{reader_task, PendingMap};
use crate::error::ClientError;

/// Write a [`ResponseEnvelope`] as a length-prefixed frame into `writer`.
async fn write_response(writer: &mut (impl AsyncWriteExt + Unpin), rid: u32, payload: &[u8]) {
    let env = ResponseEnvelope::ok(Some(rid), payload.to_vec());
    let bytes = env.to_msgpack().expect("encode response envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

/// Write an [`ErrorEnvelope`] as a length-prefixed frame into `writer`.
async fn write_error(writer: &mut (impl AsyncWriteExt + Unpin), rid: Option<u32>, error: &str) {
    let env = ErrorEnvelope::new(rid, error);
    let bytes = env.to_msgpack().expect("encode error envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

/// Write a garbage frame that cannot be decoded as either envelope type.
async fn write_garbage(writer: &mut (impl AsyncWriteExt + Unpin)) {
    let garbage = b"\xde\xad\xbe\xef not valid msgpack envelope";
    write_frame(writer, garbage).await.expect("write_frame");
}

// ─── Helpers ────────────────────────────────────────────────────────────────

#[allow(clippy::type_complexity)]
fn make_pending() -> (
    PendingMap,
    oneshot::Receiver<Result<Vec<u8>, ClientError>>,
    oneshot::Receiver<Result<Vec<u8>, ClientError>>,
) {
    let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();
    {
        let mut map = pending.lock().unwrap();
        map.insert(1, tx1);
        map.insert(2, tx2);
    }
    (pending, rx1, rx2)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Responses for rid=2 and rid=1 arrive in the wrong order; both callers
/// must receive their correct payloads.
#[tokio::test]
async fn demux_out_of_order_responses() {
    let (pending, rx1, rx2) = make_pending();
    let closed = Arc::new(AtomicBool::new(false));

    let (mut writer, reader) = tokio::io::duplex(4096);

    // Spawn the reader task.
    let task = tokio::spawn(reader_task(reader, pending, closed.clone()));

    // Send rid=2 first, then rid=1.
    write_response(&mut writer, 2, b"payload-2").await;
    write_response(&mut writer, 1, b"payload-1").await;

    // Close the writer end so reader_task sees EOF and exits.
    writer.shutdown().await.expect("shutdown writer");

    task.await.expect("reader_task panicked");

    let result1 = rx1.await.expect("rx1 dropped");
    let result2 = rx2.await.expect("rx2 dropped");

    assert_eq!(result1.unwrap(), b"payload-1");
    assert_eq!(result2.unwrap(), b"payload-2");
}

/// A frame without a rid (rid == None) must be silently dropped; the
/// two waiting callers must still receive their responses.
#[tokio::test]
async fn demux_frame_without_rid_is_dropped() {
    let (pending, rx1, rx2) = make_pending();
    let closed = Arc::new(AtomicBool::new(false));

    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(reader, pending, closed.clone()));

    // Inject a frame without a rid before the real responses.
    let no_rid_env = ResponseEnvelope::ok(None, b"push-notification".to_vec());
    let no_rid_bytes = no_rid_env.to_msgpack().unwrap();
    write_frame(&mut writer, &no_rid_bytes)
        .await
        .expect("write push frame");

    write_response(&mut writer, 1, b"p1").await;
    write_response(&mut writer, 2, b"p2").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    assert_eq!(rx1.await.unwrap().unwrap(), b"p1");
    assert_eq!(rx2.await.unwrap().unwrap(), b"p2");
}

/// A garbage frame that cannot be decoded as either envelope must be dropped;
/// remaining waiters still get their responses.
#[tokio::test]
async fn demux_garbage_frame_is_dropped() {
    let (pending, rx1, rx2) = make_pending();
    let closed = Arc::new(AtomicBool::new(false));

    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(reader, pending, closed.clone()));

    write_garbage(&mut writer).await;
    write_response(&mut writer, 1, b"ok1").await;
    write_response(&mut writer, 2, b"ok2").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    assert_eq!(rx1.await.unwrap().unwrap(), b"ok1");
    assert_eq!(rx2.await.unwrap().unwrap(), b"ok2");
}

/// On EOF, every in-flight waiter must receive `ClientError::ConnectionClosed`.
#[tokio::test]
async fn demux_eof_drains_all_pending() {
    let (pending, rx1, rx2) = make_pending();
    let closed = Arc::new(AtomicBool::new(false));

    let (writer, reader) = tokio::io::duplex(4096);

    // Drop the writer immediately — reader sees EOF right away.
    drop(writer);

    let task = tokio::spawn(reader_task(reader, pending, closed.clone()));
    task.await.expect("reader_task panicked");

    // Both waiters should have gotten ConnectionClosed.
    let err1 = rx1.await.unwrap().unwrap_err();
    let err2 = rx2.await.unwrap().unwrap_err();
    assert!(
        matches!(err1, ClientError::ConnectionClosed),
        "expected ConnectionClosed, got {err1:?}"
    );
    assert!(
        matches!(err2, ClientError::ConnectionClosed),
        "expected ConnectionClosed, got {err2:?}"
    );

    // `closed` flag must be set.
    assert!(closed.load(Ordering::Acquire));
}

/// An error envelope (server-level error) is routed to the matching waiter
/// as a `ClientError::Protocol`.
#[tokio::test]
async fn demux_error_envelope_routed_to_waiter() {
    let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
    let (tx, rx) = oneshot::channel();
    pending.lock().unwrap().insert(7, tx);

    let closed = Arc::new(AtomicBool::new(false));
    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(reader, pending, closed.clone()));

    write_error(&mut writer, Some(7), "session_expired").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    let err = rx.await.unwrap().unwrap_err();
    assert!(
        matches!(err, ClientError::Protocol(ref msg) if msg.contains("session_expired")),
        "unexpected error: {err:?}"
    );
}

/// A late response for a rid that no longer has a waiter (e.g., timed-out
/// or cancelled request) must be silently dropped — no panic, no hang.
#[tokio::test]
async fn demux_late_response_for_unknown_rid_is_dropped() {
    // Only rid=1 is registered; server sends rid=99 (orphan) + rid=1.
    let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
    let (tx1, rx1) = oneshot::channel();
    pending.lock().unwrap().insert(1, tx1);

    let closed = Arc::new(AtomicBool::new(false));
    let (mut writer, reader) = tokio::io::duplex(4096);

    let task = tokio::spawn(reader_task(reader, pending, closed.clone()));

    // orphan frame
    write_response(&mut writer, 99, b"orphan").await;
    // valid frame
    write_response(&mut writer, 1, b"mine").await;
    writer.shutdown().await.expect("shutdown");

    task.await.expect("reader_task panicked");

    // rid=1 still got its payload correctly.
    assert_eq!(rx1.await.unwrap().unwrap(), b"mine");
}
