//! Length-prefix msgpack framing on top of WebSocket BINARY messages.
//!
//! Same wire format as `shamir-transport-tcp::framing` — each message
//! carries one frame: `[u32_be length][payload]`. We use BINARY messages
//! (not TEXT) to allow arbitrary msgpack bytes.
//!
//! For each WebSocket message we receive, the inner length prefix MUST
//! match the message body length minus 4 (defense-in-depth against
//! length-prefix tampering vs WS frame boundary).
//!
//! # Zero-length frames (transport divergence from TCP)
//!
//! A four-zero-byte BINARY message (`[u32_be 0]`) is a LEGAL empty frame
//! on WS and yields an empty payload. On the TCP sibling
//! (`shamir-transport-tcp::framing`) a zero length prefix means graceful
//! peer close (`PeerClose`). The wire bytes are identical; the close
//! semantics are NOT. WS peers signal close with a WebSocket Close frame
//! ([`WsFrameError::PeerClose`]) instead.
//!
//! # Split halves
//!
//! [`ws_recv_into_stream`] and [`ws_send_sink`] operate on the individual
//! read/write halves produced by [`futures_util::StreamExt::split`].  Use
//! them when you need to hand ownership of each direction to a separate
//! task (future duplex request loops).  The whole-stream variants
//! ([`ws_recv_into`] / [`ws_send`]) are thin wrappers that borrow the
//! whole stream and delegate here.

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Default WS frame ceiling (16 MiB per spec §8 `MAX_FRAME_SIZE_DATA`).
pub const MAX_WS_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Errors raised by ws_send / ws_recv.
#[derive(Debug, Error)]
pub enum WsFrameError {
    /// Underlying WebSocket I/O error.
    #[error("ws io: {0}")]
    Io(#[from] tokio_tungstenite::tungstenite::Error),
    /// Peer sent a graceful Close frame.
    #[error("peer requested close")]
    PeerClose,
    /// Inner length prefix doesn't match WS message body length.
    #[error("length prefix mismatch: declared={declared}, actual={actual}")]
    LengthMismatch {
        /// Length declared by the inner u32_be prefix.
        declared: usize,
        /// Body length excluding the prefix, or total message length if shorter than four bytes.
        actual: usize,
    },
    /// Frame larger than `MAX_WS_FRAME_SIZE`.
    #[error("frame too large: {actual} > {max}")]
    TooLarge {
        /// Declared length.
        actual: usize,
        /// Configured cap.
        max: usize,
    },
    /// Received a non-binary WebSocket message (TEXT, PING, etc. handled
    /// elsewhere; payload-bearing messages MUST be BINARY).
    #[error("expected binary message, got: {0}")]
    NonBinaryMessage(String),
    /// RV-TRANSPORT-WS-B8ED1366: too many consecutive control frames
    /// (ping flood) without any data frame.
    #[error("control-frame flood: {count} frames, {bytes} bytes")]
    ControlFlood {
        /// Number of control frames skipped since the last data frame.
        count: usize,
        /// Total control-frame payload bytes consumed.
        bytes: usize,
    },
}

/// Send one frame as a WebSocket BINARY message.
///
/// Concatenates `len_be || payload` into a single message body — same as
/// `shamir-transport-tcp::write_frame` Optim #7 single-syscall pattern.
///
/// Delegates to [`ws_send_sink`]; see its docs for the wire format.
pub async fn ws_send<S>(stream: &mut WebSocketStream<S>, payload: &[u8]) -> Result<(), WsFrameError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws_send_sink(stream, payload).await
}

/// Receive one frame, returning the payload as an owned Vec.
///
/// For high-throughput callers prefer [`ws_recv_into`] which writes into
/// a caller-supplied scratch buffer.
pub async fn ws_recv<S>(
    stream: &mut WebSocketStream<S>,
    max_frame_size: usize,
) -> Result<Vec<u8>, WsFrameError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut buf = Vec::new();
    ws_recv_into(stream, max_frame_size, &mut buf).await?;
    Ok(buf)
}

/// Receive one frame into a caller-supplied buffer (zero-allocation steady
/// state).
///
/// Delegates to [`ws_recv_into_stream`]; see its docs for the wire format.
pub async fn ws_recv_into<S>(
    stream: &mut WebSocketStream<S>,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
) -> Result<(), WsFrameError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws_recv_into_stream(stream, max_frame_size, buf).await
}

// ---------------------------------------------------------------------------
// Generic half-stream variants — work on the individual read/write halves
// produced by `futures_util::StreamExt::split` or directly on a whole
// `WebSocketStream` (which itself implements both `Stream` and `Sink`).
// ---------------------------------------------------------------------------

/// Send one frame on any `Sink<Message>`.
///
/// Concatenates `len_be || payload` and delivers it as a single BINARY
/// message so the inner length prefix matches the WS message body length
/// minus 4 (defence-in-depth invariant).
pub async fn ws_send_sink<W>(sink: &mut W, payload: &[u8]) -> Result<(), WsFrameError>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    // Cap BEFORE narrowing to u32 or allocating: lengths ≥ 2^32 would
    // wrap the prefix (RV-TRANSPORT-WS-624F81B1).
    if payload.len() > MAX_WS_FRAME_SIZE {
        return Err(WsFrameError::TooLarge {
            actual: payload.len(),
            max: MAX_WS_FRAME_SIZE,
        });
    }
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    sink.send(Message::Binary(buf.into())).await?;
    Ok(())
}

/// Send one frame from an OWNED, already length-prefixed buffer
/// (`[u32_be length][payload]`) — RV-TRANSPORT-WS-B7D06174.
///
/// Ownership lets the buffer become the BINARY message body directly: no
/// fresh allocation, no payload copy (the `ws_send_sink` path allocates and
/// copies the whole payload per frame). Validates the prefix and the frame
/// cap before handing the buffer to the sink.
pub async fn ws_send_sink_prereserved<W>(sink: &mut W, buf: Vec<u8>) -> Result<(), WsFrameError>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    if buf.len() < 4 {
        return Err(WsFrameError::LengthMismatch {
            declared: 0,
            actual: buf.len(),
        });
    }
    let declared = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let payload_len = buf.len() - 4;
    if declared != payload_len {
        return Err(WsFrameError::LengthMismatch {
            declared,
            actual: payload_len,
        });
    }
    if payload_len > MAX_WS_FRAME_SIZE {
        return Err(WsFrameError::TooLarge {
            actual: payload_len,
            max: MAX_WS_FRAME_SIZE,
        });
    }
    sink.send(Message::Binary(buf.into())).await?;
    Ok(())
}

/// Receive one frame from any `Stream<Item = Result<Message, …>>` into a
/// caller-supplied buffer (zero-allocation steady state).
///
/// All frame validation (length prefix, size cap, non-binary rejection) is
/// identical to [`ws_recv_into`] — this is the shared implementation both
/// whole-stream and split-half variants delegate to.
pub async fn ws_recv_into_stream<R>(
    stream: &mut R,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
) -> Result<(), WsFrameError>
where
    R: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    ws_recv_into_stream_inner(stream, max_frame_size, buf, None).await
}

pub async fn ws_recv_into_stream_observing<R>(
    stream: &mut R,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
    heartbeat: &HeartbeatState,
) -> Result<(), WsFrameError>
where
    R: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    ws_recv_into_stream_inner(stream, max_frame_size, buf, Some(heartbeat)).await
}

async fn ws_recv_into_stream_inner<R>(
    stream: &mut R,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
    heartbeat: Option<&HeartbeatState>,
) -> Result<(), WsFrameError>
where
    R: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    // RV-TRANSPORT-WS-B8ED1366: bound control-only work so a peer can't
    // keep the receive loop spinning forever with control frames.
    let mut control_count: usize = 0;
    let mut control_bytes: usize = 0;
    loop {
        let msg = match stream.next().await {
            Some(Ok(m)) => m,
            Some(Err(e)) => return Err(e.into()),
            None => return Err(WsFrameError::PeerClose),
        };
        match msg {
            Message::Binary(bytes) => {
                if bytes.len() < 4 {
                    return Err(WsFrameError::LengthMismatch {
                        declared: 0,
                        actual: bytes.len(),
                    });
                }
                let declared =
                    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                let body = &bytes[4..];
                if declared != body.len() {
                    return Err(WsFrameError::LengthMismatch {
                        declared,
                        actual: body.len(),
                    });
                }
                if declared > max_frame_size {
                    return Err(WsFrameError::TooLarge {
                        actual: declared,
                        max: max_frame_size,
                    });
                }
                buf.clear();
                buf.extend_from_slice(body);
                return Ok(());
            }
            Message::Close(_) => return Err(WsFrameError::PeerClose),
            // Ping/Pong handled by tungstenite automatically; loop and
            // wait for next "real" message.
            Message::Ping(p) => {
                control_count += 1;
                control_bytes += p.len();
                check_control_budget(control_count, control_bytes)?;
            }
            Message::Pong(p) => {
                if is_matched_heartbeat_pong(heartbeat, &p) {
                    continue;
                }
                control_count += 1;
                control_bytes += p.len();
                check_control_budget(control_count, control_bytes)?;
            }
            Message::Text(t) => {
                return Err(WsFrameError::NonBinaryMessage(format!(
                    "TEXT len={}",
                    t.len()
                )))
            }
            Message::Frame(_) => {
                control_count += 1;
                check_control_budget(control_count, control_bytes)?;
            }
        }
    }
}

/// Control-frame loop budget (RV-TRANSPORT-WS-B8ED1366).
const MAX_CONTROL_FRAMES: usize = 64;
/// Control-payload byte budget across one receive call.
const MAX_CONTROL_BYTES: usize = 8 * 1024;

fn check_control_budget(count: usize, bytes: usize) -> Result<(), WsFrameError> {
    if count > MAX_CONTROL_FRAMES || bytes > MAX_CONTROL_BYTES {
        return Err(WsFrameError::ControlFlood { count, bytes });
    }
    Ok(())
}

/// Per-connection heartbeat correlation state shared between WS halves.
#[derive(Clone, Default)]
pub struct HeartbeatState(Arc<State>);

#[derive(Default)]
struct State {
    last_sent: AtomicU64,
    last_acked: AtomicU64,
    reader_parked: AtomicBool,
}

impl HeartbeatState {
    pub fn next_ping(&self) -> ([u8; 8], u64) {
        let sequence = self
            .0
            .last_sent
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);
        self.0.last_sent.store(sequence, Ordering::Release);
        (sequence.to_be_bytes(), sequence)
    }

    pub fn observe_pong(&self, payload: &[u8]) -> bool {
        if payload.len() != 8 {
            return false;
        }
        let sequence = u64::from_be_bytes(payload.try_into().expect("8-byte payload"));
        let sent = self.0.last_sent.load(Ordering::Acquire);
        if sequence == 0 || sequence != sent {
            return false;
        }
        let mut acked = self.0.last_acked.load(Ordering::Acquire);
        loop {
            if sequence <= acked {
                return false;
            }
            match self.0.last_acked.compare_exchange_weak(
                acked,
                sequence,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => acked = observed,
            }
        }
    }

    pub fn pending(&self) -> bool {
        let sent = self.0.last_sent.load(Ordering::Acquire);
        sent != 0 && self.0.last_acked.load(Ordering::Acquire) < sent
    }
    pub fn reader_parked(&self, parked: bool) {
        self.0.reader_parked.store(parked, Ordering::Release);
    }
    pub fn is_reader_parked(&self) -> bool {
        self.0.reader_parked.load(Ordering::Acquire)
    }
}

pub fn is_matched_heartbeat_pong(state: Option<&HeartbeatState>, payload: &[u8]) -> bool {
    state.is_some_and(|state| state.observe_pong(payload))
}
