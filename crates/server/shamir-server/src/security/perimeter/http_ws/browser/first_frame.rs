//! Bounded first browser WebSocket message parser.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::{timeout_at, Instant};

/// Maximum encoded browser token message size.
pub(crate) const FIRST_FRAME_MAX: usize = 256;
/// Absolute deadline for the first browser frame and permitted controls.
pub(crate) const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONTROLS: usize = 8;

/// Read the first masked final binary frame and decode exactly `{token: string}`.
pub(crate) async fn read_browser_token<R: AsyncRead + Unpin>(reader: &mut R) -> Result<String, ()> {
    let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
    let mut controls = 0usize;
    loop {
        let mut header = [0u8; 2];
        read_exact(reader, &mut header, deadline).await?;
        let (first, second) = (header[0], header[1]);
        let opcode = first & 0x0f;
        let short_len = usize::from(second & 0x7f);
        if first & 0x70 != 0 || second & 0x80 == 0 {
            return Err(());
        }
        if opcode & 0x08 != 0 {
            if first & 0x80 == 0 || short_len >= 126 || !(0x8..=0xA).contains(&opcode) {
                return Err(());
            }
            controls += 1;
            if controls > MAX_CONTROLS {
                return Err(());
            }
            let mut mask = [0; 4];
            read_exact(reader, &mut mask, deadline).await?;
            let mut payload = [0; 125];
            read_exact(reader, &mut payload[..short_len], deadline).await?;
            for (i, byte) in payload[..short_len].iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
            if opcode == 0x8 {
                return Err(());
            }
            continue;
        }
        if opcode != 0x2 || first & 0x80 == 0 || short_len == 127 {
            return Err(());
        }
        let length = if short_len == 126 {
            let mut raw = [0; 2];
            read_exact(reader, &mut raw, deadline).await?;
            let value = usize::from(u16::from_be_bytes(raw));
            if value < 126 {
                return Err(());
            }
            value
        } else {
            short_len
        };
        if length > FIRST_FRAME_MAX {
            return Err(());
        }
        let mut mask = [0; 4];
        read_exact(reader, &mut mask, deadline).await?;
        let mut payload = vec![0; length];
        read_exact(reader, &mut payload, deadline).await?;
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
        let token = decode_token_map(&payload).ok_or(())?;
        if token.len() != 71 || !token.is_ascii() {
            return Err(());
        }
        return Ok(token);
    }
}

async fn read_exact<R: AsyncRead + Unpin>(
    reader: &mut R,
    bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), ()> {
    timeout_at(deadline, reader.read_exact(bytes))
        .await
        .map_err(|_| ())?
        .map(|_| ())
        .map_err(|_| ())
}

/// Decodes exactly a one-entry map `{"token": <str>}` with no trailing bytes.
fn decode_token_map(payload: &[u8]) -> Option<String> {
    let rest = payload.strip_prefix(&[0x81])?;
    let (key, rest) = read_str(rest)?;
    if key != b"token" {
        return None;
    }
    let (value, rest) = read_str(rest)?;
    if !rest.is_empty() {
        return None;
    }
    std::str::from_utf8(value).ok().map(str::to_owned)
}

/// Reads one MessagePack string; binary and non-string values are rejected.
fn read_str(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&marker, rest) = input.split_first()?;
    let (len, rest) = match marker {
        0xa0..=0xbf => (usize::from(marker & 0x1f), rest),
        0xd9 => {
            let (&len, rest) = rest.split_first()?;
            (usize::from(len), rest)
        }
        0xda => {
            let (len, rest) = rest.split_first_chunk::<2>()?;
            (usize::from(u16::from_be_bytes(*len)), rest)
        }
        0xdb => {
            let (len, rest) = rest.split_first_chunk::<4>()?;
            (usize::try_from(u32::from_be_bytes(*len)).ok()?, rest)
        }
        _ => return None,
    };
    (rest.len() >= len).then(|| rest.split_at(len))
}
