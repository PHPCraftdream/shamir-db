//! Length-prefix msgpack framing per TRANSPORT_TCP §2.
//!
//! Wire format:
//! ```text
//! [length: u32 BE][msgpack: length bytes]
//! ```
//!
//! `length == 0` is reserved for the graceful-close indicator (spec §2),
//! so the writers reject empty payloads instead of encoding them.

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Default upper bound on frame size — 16 MiB per spec §8 `MAX_FRAME_SIZE_DATA`.
pub const MAX_FRAME_SIZE_DEFAULT: usize = 16 * 1024 * 1024;

/// Framing errors.
#[derive(Debug, Error)]
pub enum FrameError {
    /// Underlying TCP/TLS I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Frame larger than the negotiated maximum.
    #[error("frame too large: {actual} > {max}")]
    TooLarge {
        /// Length declared by the peer.
        actual: usize,
        /// Local cap.
        max: usize,
    },
    /// Peer sent a graceful-close (length == 0).
    #[error("peer requested close")]
    PeerClose,
    /// Peer ended the stream while the 4-byte length header was being read
    /// (clean EOF at a frame boundary) — an abrupt disconnect, distinct
    /// from a graceful close and from a mid-body truncation (`Io`).
    #[error("peer eof")]
    PeerEof,
    /// Malformed caller buffer/prefix contract.
    #[error("malformed preframed buffer: {0}")]
    MalformedBuffer(&'static str),
    /// Empty payload — length 0 is reserved for the graceful-close sentinel
    /// (spec §2), so writers refuse to emit it as a data frame.
    #[error("empty payload: length 0 is the close sentinel")]
    EmptyPayload,
}

/// Read the 4-byte length header. EOF before the first header byte is
/// [`FrameError::PeerEof`] (peer disconnected between frames); EOF after one
/// to three header bytes is a truncated frame (`Io(UnexpectedEof)`).
async fn read_len_header<R: AsyncRead + Unpin>(reader: &mut R) -> Result<usize, FrameError> {
    let mut len_buf = [0u8; 4];
    let mut got = 0;
    while got < len_buf.len() {
        let n = reader.read(&mut len_buf[got..]).await?;
        if n == 0 {
            return Err(if got == 0 {
                FrameError::PeerEof
            } else {
                FrameError::Io(std::io::ErrorKind::UnexpectedEof.into())
            });
        }
        got += n;
    }
    Ok(u32::from_be_bytes(len_buf) as usize)
}

/// Read one frame: `[u32_be length][bytes]`.
///
/// Any framing error may leave the stream positioned partway through a frame
/// (oversize errors consume the header but not its body). Discard the stream
/// after an error; do not retry a frame read.
///
/// Body accumulation only grows as bytes arrive; no allocation is made from
/// the declared frame length. `read_frame` still allocates its result.
/// `read_frame` and `read_frame_into` retain partial bytes locally while
/// awaiting body progress; cancellation drops that progress. Treat cancellation
/// as terminal and discard the stream rather than restarting the frame read.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_frame_size: usize,
) -> Result<Vec<u8>, FrameError> {
    let len = read_len_header(reader).await?;

    if len == 0 {
        return Err(FrameError::PeerClose);
    }
    if len > max_frame_size {
        return Err(FrameError::TooLarge {
            actual: len,
            max: max_frame_size,
        });
    }

    // Grow only as bytes arrive: the peer's declaration bounds total work but
    // must not force an up-front allocation of that full size.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while buf.len() < len {
        let remaining = len - buf.len();
        let chunk_len = remaining.min(chunk.len());
        let n = reader.read(&mut chunk[..chunk_len]).await?;
        if n == 0 {
            return Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof.into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

/// Read one frame into a caller-supplied buffer, reusing existing capacity.
///
/// On success the buffer contains exactly the payload bytes. Previous contents
/// are overwritten. On error or cancellation the stream may be mid-frame;
/// discard it rather than retrying. In particular, oversize errors consume
/// the header but do not drain the body.
///
/// This is the high-throughput variant of [`read_frame`]: it avoids the
/// per-call heap allocation by reusing the buffer's existing capacity.
/// Typical use in a per-connection request loop:
///
/// ```rust,ignore
/// let mut scratch = Vec::with_capacity(4096);
/// loop {
///     read_frame_into(&mut reader, MAX_FRAME_SIZE_DEFAULT, &mut scratch).await?;
///     handle_payload(&scratch);
/// }
/// ```
///
/// The buffer capacity grows only as payload bytes arrive, not from the
/// declared length. Use [`Vec::shrink_to_fit`] if memory is a concern.
/// Cancellation also loses the partial frame progress; discard the stream
/// rather than retrying after cancellation.
pub async fn read_frame_into<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
) -> Result<(), FrameError> {
    let len = read_len_header(reader).await?;

    if len == 0 {
        return Err(FrameError::PeerClose);
    }
    if len > max_frame_size {
        return Err(FrameError::TooLarge {
            actual: len,
            max: max_frame_size,
        });
    }

    // Keep only received bytes live; declared length never sets buffer length.
    buf.clear();
    let mut chunk = [0u8; 4096];
    while buf.len() < len {
        let remaining = len - buf.len();
        let chunk_len = remaining.min(chunk.len());
        let n = match reader.read(&mut chunk[..chunk_len]).await {
            Ok(0) => {
                buf.clear();
                return Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof.into()));
            }
            Ok(n) => n,
            Err(e) => {
                buf.clear();
                return Err(e.into());
            }
        };
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(())
}

/// Write one frame: prepends `u32_be length` then payload.
///
/// **Optim #7:** length and payload are concatenated into ONE buffer and
/// written via a single `write_all`. With `tokio_rustls::TlsStream` two
/// separate `write_all` calls produce two TLS records (each ~22 bytes of
/// header + tag overhead) — combining them halves the wire overhead for
/// small responses. Allocates one temporary `Vec<u8>` of size `4 + N`; for
/// callers wanting zero allocation see [`write_frame_into`] which writes
/// into a caller-supplied scratch buffer.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), FrameError> {
    if payload.len() > MAX_FRAME_SIZE_DEFAULT {
        return Err(FrameError::TooLarge {
            actual: payload.len(),
            max: MAX_FRAME_SIZE_DEFAULT,
        });
    }
    if payload.is_empty() {
        return Err(FrameError::EmptyPayload);
    }
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    writer.write_all(&buf).await?;
    writer.flush().await?;
    Ok(())
}

/// Same as [`write_frame`] but writes into a caller-supplied scratch
/// buffer that's reused across calls (zero-allocation in steady state).
///
/// `scratch` is `clear()`-ed first; on return it contains the framed
/// bytes that were sent (caller may keep capacity for the next call).
/// The buffer's capacity grows monotonically to the high-water mark.
///
/// Pair with [`read_frame_into`] for a fully-pooled per-connection
/// request loop:
///
/// ```rust,ignore
/// let mut read_buf = Vec::with_capacity(4096);
/// let mut write_buf = Vec::with_capacity(4096);
/// loop {
///     read_frame_into(&mut r, MAX_FRAME_SIZE_DEFAULT, &mut read_buf).await?;
///     let response = handle(&read_buf);
///     write_frame_into(&mut w, &response, &mut write_buf).await?;
/// }
/// ```
pub async fn write_frame_into<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    scratch: &mut Vec<u8>,
) -> Result<(), FrameError> {
    if payload.len() > MAX_FRAME_SIZE_DEFAULT {
        return Err(FrameError::TooLarge {
            actual: payload.len(),
            max: MAX_FRAME_SIZE_DEFAULT,
        });
    }
    if payload.is_empty() {
        return Err(FrameError::EmptyPayload);
    }
    let len = payload.len() as u32;
    scratch.clear();
    scratch.reserve(4 + payload.len());
    scratch.extend_from_slice(&len.to_be_bytes());
    scratch.extend_from_slice(payload);
    writer.write_all(scratch).await?;
    writer.flush().await?;
    Ok(())
}

/// Send a graceful-close indicator (`length=0`).
pub async fn write_close<W: AsyncWrite + Unpin>(writer: &mut W) -> Result<(), FrameError> {
    writer.write_all(&[0u8; 4]).await?;
    writer.flush().await?;
    Ok(())
}

/// Write one frame from an ALREADY length-prefixed buffer, skipping the
/// internal memcpy that [`write_frame_into`] does to prepend the 4-byte
/// length prefix.
///
/// `buf` must start with a 4-byte big-endian `u32` length prefix followed
/// by exactly that many payload bytes — i.e. `buf.len() == 4 + payload_len`
/// and `u32::from_be_bytes(buf[0..4]) == payload_len as u32`.
///
/// This is the zero-copy-encoding variant for callers that serialize the
/// response payload DIRECTLY into a buffer that already has the 4-byte
/// length prefix reserved (push 4 placeholder bytes, serialize, patch the
/// length in-place). For large responses (multi-MB SELECT results) this
/// avoids a full-payload memcpy into a scratch buffer.
///
/// # Correctness contract
///
/// The caller MUST ensure `buf[0..4]` is a valid BE `u32` equal to
/// `buf.len() - 4`. Violating this produces a malformed frame on the wire.
///
/// Returns [`FrameError::MalformedBuffer`] for short buffers or a prefix/body
/// mismatch, [`FrameError::TooLarge`] only when the valid declared payload
/// exceeds the supported frame limit.
pub async fn write_frame_prereserved<W: AsyncWrite + Unpin>(
    writer: &mut W,
    buf: &[u8],
) -> Result<(), FrameError> {
    // Defensive: verify the length prefix matches the buffer size. This
    // catches caller bugs that would silently produce corrupt frames.
    // The check is O(1) (two loads + one comparison) — negligible vs. the
    // I/O cost of the `write_all`.
    if buf.len() < 4 {
        return Err(FrameError::MalformedBuffer("prefix requires four bytes"));
    }
    let declared_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if declared_len == 0 {
        return Err(FrameError::EmptyPayload);
    }
    // The payload length on the wire must match what's in the buffer.
    let actual_payload = buf.len() - 4;
    if declared_len > MAX_FRAME_SIZE_DEFAULT {
        return Err(FrameError::TooLarge {
            actual: declared_len,
            max: MAX_FRAME_SIZE_DEFAULT,
        });
    }
    if declared_len != actual_payload {
        return Err(FrameError::MalformedBuffer(
            "declared length does not match payload length",
        ));
    }
    writer.write_all(buf).await?;
    writer.flush().await?;
    Ok(())
}
