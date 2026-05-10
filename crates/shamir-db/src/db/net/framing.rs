//! Length-prefixed MessagePack framing.
//!
//! Wire format:
//!   [length: 4 bytes BE][msgpack payload: length bytes]
//!
//! MAX_FRAME_SIZE enforced before allocation.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024; // 16 MB

/// Read one length-prefixed msgpack frame.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Vec<u8>, FrameError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            FrameError::ConnectionClosed
        } else {
            FrameError::Io(e)
        }
    })?;

    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge(len));
    }

    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await.map_err(FrameError::Io)?;

    Ok(payload)
}

/// Write one length-prefixed msgpack frame.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), FrameError> {
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await.map_err(FrameError::Io)?;
    writer.write_all(payload).await.map_err(FrameError::Io)?;
    writer.flush().await.map_err(FrameError::Io)?;
    Ok(())
}

/// Serialize value to msgpack and write as frame.
pub async fn write_msg<W: AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), FrameError> {
    let payload = rmp_serde::to_vec_named(value).map_err(FrameError::Encode)?;
    write_frame(writer, &payload).await
}

/// Read frame and deserialize from msgpack.
pub async fn read_msg<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> Result<T, FrameError> {
    let payload = read_frame(reader).await?;
    rmp_serde::from_slice(&payload).map_err(FrameError::Decode)
}

#[derive(Debug)]
pub enum FrameError {
    ConnectionClosed,
    TooLarge(u32),
    Io(std::io::Error),
    Encode(rmp_serde::encode::Error),
    Decode(rmp_serde::decode::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::ConnectionClosed => write!(f, "Connection closed"),
            FrameError::TooLarge(n) => write!(f, "Frame too large: {} bytes", n),
            FrameError::Io(e) => write!(f, "IO error: {}", e),
            FrameError::Encode(e) => write!(f, "Encode error: {}", e),
            FrameError::Decode(e) => write!(f, "Decode error: {}", e),
        }
    }
}

impl std::error::Error for FrameError {}
