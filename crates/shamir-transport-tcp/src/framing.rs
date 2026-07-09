//! Length-prefix msgpack framing per TRANSPORT_TCP §2.
//!
//! Wire format:
//! ```text
//! [length: u32 BE][msgpack: length bytes]
//! ```
//!
//! `length == 0` is a graceful close indicator. Empty frames are also legal
//! at the application level (caller decides).

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
}

/// Read one frame: `[u32_be length][bytes]`.
///
/// Returns `Ok(payload)` for normal frames, [`FrameError::PeerClose`] for
/// length-zero frames, or [`FrameError::TooLarge`] if the declared length
/// exceeds `max_frame_size`.
///
/// **Allocates a fresh `Vec<u8>` per call.** For high-throughput callers
/// (per-connection request loops) prefer [`read_frame_into`] which
/// reuses a caller-supplied scratch buffer.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_frame_size: usize,
) -> Result<Vec<u8>, FrameError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len == 0 {
        return Err(FrameError::PeerClose);
    }
    if len > max_frame_size {
        return Err(FrameError::TooLarge {
            actual: len,
            max: max_frame_size,
        });
    }

    // `vec![0u8; len]` uses the `SpecFromElem` u8-specialization which
    // compiles to one `write_bytes` (memset). Significantly faster than
    // `Vec::with_capacity(len) + resize(len, 0)` which goes through the
    // generic per-element loop.
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Read one frame into a caller-supplied buffer, reusing existing capacity.
///
/// On success the buffer is resized to the frame length and contains exactly
/// the payload bytes. Previous contents are overwritten.
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
/// The buffer's capacity grows monotonically to the high-water mark of
/// frames seen so far. Use [`Vec::shrink_to_fit`] periodically if memory is
/// a concern.
///
/// Implementation: `unsafe set_len` after `reserve` lets us skip the
/// zero-fill (`Vec::resize(_, 0)` would zero-init via the generic
/// per-element loop, which is slower than memset for `u8`). Safety: we
/// allocate `len` bytes of capacity, then `read_exact` fully overwrites
/// them — uninitialized bytes are never observed by safe code.
pub async fn read_frame_into<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_frame_size: usize,
    buf: &mut Vec<u8>,
) -> Result<(), FrameError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len == 0 {
        return Err(FrameError::PeerClose);
    }
    if len > max_frame_size {
        return Err(FrameError::TooLarge {
            actual: len,
            max: max_frame_size,
        });
    }

    buf.clear();
    // SAFETY: `reserve(len)` ensured `capacity >= len`. `set_len(len)`
    // exposes uninitialized bytes — we IMMEDIATELY pass `&mut buf[..]` to
    // `read_exact` which fully overwrites all `len` bytes or returns
    // `Err`. The uninit bytes are never read by safe code: on success the
    // buffer is fully initialized; on error we never observe `&buf[..]`
    // (the function returns the error directly without exposing `buf`).
    buf.reserve(len);
    #[allow(clippy::uninit_vec)] // see SAFETY comment above
    unsafe {
        buf.set_len(len);
    }
    if let Err(e) = reader.read_exact(buf).await {
        // On error reset len so the caller doesn't see uninit bytes.
        // (This is defense-in-depth; the function returns Err so the
        // caller shouldn't access buf, but truncating to 0 ensures even
        // a misuse can't read uninitialized memory through the buffer.)
        buf.clear();
        return Err(e.into());
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
pub async fn write_frame_prereserved<W: AsyncWrite + Unpin>(
    writer: &mut W,
    buf: &[u8],
) -> Result<(), FrameError> {
    // Defensive: verify the length prefix matches the buffer size. This
    // catches caller bugs that would silently produce corrupt frames.
    // The check is O(1) (two loads + one comparison) — negligible vs. the
    // I/O cost of the `write_all`.
    if buf.len() < 4 {
        return Err(FrameError::TooLarge {
            actual: 0,
            max: MAX_FRAME_SIZE_DEFAULT,
        });
    }
    let declared_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    // The payload length on the wire must match what's in the buffer.
    let actual_payload = buf.len() - 4;
    if declared_len != actual_payload || declared_len > MAX_FRAME_SIZE_DEFAULT {
        return Err(FrameError::TooLarge {
            actual: declared_len,
            max: MAX_FRAME_SIZE_DEFAULT,
        });
    }
    writer.write_all(buf).await?;
    writer.flush().await?;
    Ok(())
}
