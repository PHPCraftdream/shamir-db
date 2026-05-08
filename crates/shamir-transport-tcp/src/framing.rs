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
    buf.reserve(len);
    // SAFETY: `reserve(len)` ensured `capacity >= len`. `set_len(len)`
    // exposes uninitialized bytes — we IMMEDIATELY pass `&mut buf[..]` to
    // `read_exact` which fully overwrites all `len` bytes or returns
    // `Err`. The uninit bytes are never read by safe code: on success the
    // buffer is fully initialized; on error we never observe `&buf[..]`
    // (the function returns the error directly without exposing `buf`).
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
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), FrameError> {
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    if !payload.is_empty() {
        writer.write_all(payload).await?;
    }
    writer.flush().await?;
    Ok(())
}

/// Send a graceful-close indicator (`length=0`).
pub async fn write_close<W: AsyncWrite + Unpin>(writer: &mut W) -> Result<(), FrameError> {
    writer.write_all(&[0u8; 4]).await?;
    writer.flush().await?;
    Ok(())
}
