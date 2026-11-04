//! Frame decoding and the background reader task for the rid-demux mux.

use std::io::Cursor;
use std::sync::Arc;

use serde::Deserialize;

use tokio::sync::oneshot;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};

use shamir_transport_tcp::framing::{read_frame_into, FrameError, MAX_FRAME_SIZE_DEFAULT};

use crate::error::{ClientError, CloseCause};

use super::lifecycle::{Lifecycle, LifecycleHooks, ShutdownGuard};

/// Result of a demux decode: either a response payload or a transport-level
/// error string, both tagged with the correlation id.
enum DemuxResult {
    /// Success envelope: `(rid, payload_bytes)`.
    Response {
        rid: Option<u32>,
        payload: Vec<u8>,
    },
    /// Error envelope: `(rid, error_string)`.
    Error {
        rid: Option<u32>,
        error: String,
    },
    Push(PushEnvelope),
}

fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct IncomingEnvelope {
    #[serde(default)]
    rid: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_present")]
    res: Option<serde_bytes::ByteBuf>,
    #[serde(default, deserialize_with = "deserialize_present")]
    error: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    push: Option<PushKind>,
    #[serde(default)]
    sub: Option<u64>,
    #[serde(default)]
    seq: Option<u64>,
    #[serde(default)]
    data: Option<serde_bytes::ByteBuf>,
    #[serde(default)]
    gap_at: Option<u64>,
}

fn decode_frame(buf: &[u8]) -> Option<DemuxResult> {
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(buf));
    let env = IncomingEnvelope::deserialize(&mut decoder).ok()?;
    if decoder.get_ref().position() != buf.len() as u64 {
        return None;
    }
    match (env.res, env.error, env.push) {
        (Some(payload), None, None) => Some(DemuxResult::Response {
            rid: env.rid,
            payload: payload.into_vec(),
        }),
        (None, Some(error), None) => Some(DemuxResult::Error {
            rid: env.rid,
            error,
        }),
        (None, None, Some(push)) => Some(DemuxResult::Push(PushEnvelope {
            push,
            sub: env.sub?,
            seq: env.seq?,
            data: env.data.map(serde_bytes::ByteBuf::into_vec),
            gap_at: env.gap_at,
        })),
        _ => None,
    }
}

/// Await the reader task's response for `rid`, optionally bounded by a
/// per-request deadline.
///
/// - `timeout == None` → unbounded await (the original behaviour): resolve
///   only when the reader task delivers the payload, a transport error, or
///   drops the sender (connection closed).
/// - `timeout == Some(d)` → bound the await via [`tokio::time::timeout`]. On
///   elapse, the now-orphaned pending entry for `rid` is removed (so a late
///   response is cleanly dropped instead of leaking a dead sender) and
///   [`ClientError::RequestTimeout`] is returned.
///
/// Collapses the three success/error outcomes into a single
/// `Result<Vec<u8>, ClientError>`: `Ok(payload)` on success, the reader
/// task's `ClientError` on a delivered error, `ConnectionClosed` if the
/// sender was dropped, or `RequestTimeout` on deadline elapse.
///
/// The receive is raced against [`Lifecycle::closed_wait`]
/// (RV-CLIENT-B80C0D09, belt-and-suspenders): even a hypothetically
/// stranded pending entry can never hang the caller — closure unblocks it
/// and the orphaned entry is removed.
pub(crate) async fn await_pending_response(
    rx: oneshot::Receiver<Result<Vec<u8>, ClientError>>,
    timeout: Option<std::time::Duration>,
    state: &Lifecycle,
    rid: u32,
) -> Result<Vec<u8>, ClientError> {
    let recv = match timeout {
        None => tokio::select! {
            biased;
            r = rx => r,
            cause = state.closed_wait() => {
                state.pending.map.remove_sync(&rid);
                return Err(ClientError::ConnectionClosed(cause));
            }
        },
        Some(d) => tokio::select! {
            biased;
            r = tokio::time::timeout(d, rx) => match r {
                Ok(res) => res,
                Err(_elapsed) => {
                    state.pending.map.remove_sync(&rid);
                    return Err(ClientError::RequestTimeout(d));
                }
            },
            cause = state.closed_wait() => {
                state.pending.map.remove_sync(&rid);
                return Err(ClientError::ConnectionClosed(cause));
            }
        },
    };
    match recv {
        Ok(inner) => inner,
        // oneshot sender was dropped — reader task died.
        Err(_) => Err(ClientError::ConnectionClosed(
            state.cause().unwrap_or(CloseCause::ReaderCancelled),
        )),
    }
}

/// Background reader loop.  Owns the read half; demuxes frames to pending
/// waiters and live/early subscription channels via the coherent subscription mutex.
///
/// A [`ShutdownGuard`] is installed BEFORE the loop: EOF and I/O errors set
/// their cause explicitly; abort/cancel/panic fall back to
/// [`CloseCause::ReaderCancelled`]. The guard's `Drop` runs the idempotent
/// [`Lifecycle::shutdown`], so EVERY exit path closes subscriptions and
/// drains pending requests.
pub(crate) async fn reader_task<R>(
    mut reader: R,
    state: Lifecycle,
    guard: ShutdownGuard,
    hooks: LifecycleHooks,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    // T-tcp-1: reuse a single buffer across frames — avoids per-frame heap
    // allocation. Capacity grows to the high-water mark and is never freed
    // until the task exits. Borrow is fully released before every `.await`
    // below that is not `read_frame_into` itself, so no lifetime crosses an
    // unrelated await point.
    let mut frame_buf: Vec<u8> = Vec::with_capacity(4096);
    let cause: CloseCause = loop {
        match read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut frame_buf).await {
            Ok(()) => {}
            Err(FrameError::PeerEof) => {
                // Peer dropped the stream mid-header — same cause as a
                // graceful close from the reader's point of view.
                break CloseCause::PeerEof;
            }
            Err(FrameError::PeerClose) => {
                // Graceful close from server.
                break CloseCause::PeerEof;
            }
            Err(e) => {
                tracing::debug!("reader_task: read_frame error: {e}");
                let s: Arc<str> = Arc::from(e.to_string());
                break CloseCause::Io(s);
            }
        }
        let result = match decode_frame(&frame_buf) {
            Some(r @ (DemuxResult::Response { .. } | DemuxResult::Error { .. })) => r,
            Some(DemuxResult::Push(envelope)) => {
                // Per-key registry transition owns the routing decision and
                // keeps the sender operation ordered against early flush.
                state.subs.route(envelope);
                continue;
            }
            None => {
                tracing::warn!("reader_task: invalid envelope ({} bytes)", frame_buf.len());
                break CloseCause::Protocol(Arc::from("invalid response envelope"));
            }
        };

        let (rid, outcome) = match result {
            DemuxResult::Response { rid, payload } => (rid, Ok(payload)),
            DemuxResult::Error { rid, error } => (rid, Err(ClientError::Server { code: error })),
            DemuxResult::Push(_) => unreachable!(),
        };

        let rid = match rid {
            Some(r) => r,
            None => {
                // Response/error envelope without rid — unusual but harmless.
                tracing::debug!("reader_task: frame without rid, dropping");
                continue;
            }
        };

        let sender = state.pending.map.remove_sync(&rid).map(|(_, tx)| tx);

        match sender {
            Some(tx) => {
                // Ignore the error: the waiter may have been cancelled.
                let _ = tx.send(outcome);
            }
            None => {
                // Late response for a cancelled/timed-out request.
                tracing::debug!("reader_task: no waiter for rid={rid}, dropping");
            }
        }
    };

    guard.set(cause.clone());
    #[cfg(test)]
    if let Some(hook) = &hooks.reader_before_shutdown {
        hook.fire().await;
    }
    #[cfg(not(test))]
    let _ = &hooks; // zero-sized in production builds
                    // Dropping the guard runs the idempotent shutdown with `cause`.
    drop(guard);
}
