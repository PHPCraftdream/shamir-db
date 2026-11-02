//! Bounded first browser WebSocket message parser.

use std::time::Duration;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
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
            if first & 0x80 == 0 || short_len >= 126 || opcode < 0x8 || opcode > 0xA {
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
        let token: StrictToken = rmp_serde::from_slice(&payload).map_err(|_| ())?;
        if token.0.len() != 71 || !token.0.is_ascii() {
            return Err(());
        }
        return Ok(token.0);
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

struct StrictToken(String);

impl<'de> Deserialize<'de> for StrictToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TokenVisitor;
        impl<'de> Visitor<'de> for TokenVisitor {
            type Value = StrictToken;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a one-entry token map")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let key: String = map
                    .next_key()?
                    .ok_or_else(|| serde::de::Error::custom("missing token"))?;
                if key != "token" {
                    return Err(serde::de::Error::custom("unknown map key"));
                }
                let token = map.next_value::<String>()?;
                if map.next_key::<String>()?.is_some() {
                    return Err(serde::de::Error::custom("extra map key"));
                }
                Ok(StrictToken(token))
            }
        }
        deserializer.deserialize_map(TokenVisitor)
    }
}
