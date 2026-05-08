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

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
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
