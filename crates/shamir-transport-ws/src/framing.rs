//! Length-prefix msgpack framing on top of WebSocket BINARY messages.
//!
//! Same wire format as `shamir-transport-tcp::framing` — each message
//! carries one frame: `[u32_be length][payload]`. We use BINARY messages
//! (not TEXT) to allow arbitrary msgpack bytes.
//!
//! For each WebSocket message we receive, the inner length prefix MUST
//! match the message body length minus 4 (defense-in-depth against
//! length-prefix tampering vs WS frame boundary).

use futures_util::{SinkExt, StreamExt};
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
        /// Actual length of the WS message body minus 4.
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
}

/// Send one frame as a WebSocket BINARY message.
///
/// Concatenates `len_be || payload` into a single message body — same as
/// `shamir-transport-tcp::write_frame` Optim #7 single-syscall pattern.
pub async fn ws_send<S>(stream: &mut WebSocketStream<S>, payload: &[u8]) -> Result<(), WsFrameError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    stream.send(Message::Binary(buf.into())).await?;
    Ok(())
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
pub async fn ws_recv_into<S>(
    stream: &mut WebSocketStream<S>,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
) -> Result<(), WsFrameError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
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
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Text(t) => {
                return Err(WsFrameError::NonBinaryMessage(format!(
                    "TEXT len={}",
                    t.len()
                )))
            }
            Message::Frame(_) => continue,
        }
    }
}
