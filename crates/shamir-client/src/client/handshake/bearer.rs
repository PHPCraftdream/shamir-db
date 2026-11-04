use std::fmt;
use std::io::Cursor;

use futures::StreamExt;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::client::transport::WsPeerClose;
use crate::error::{ClientError, CloseCause};

const SID_LEN: usize = 32;

/// Server-issued session identity of a token WebSocket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenBinding {
    pub(crate) sid: [u8; 32],
    pub(crate) expires_at_ns: u64,
}

/// A msgpack `bin` of exactly `SID_LEN` bytes; `str` and arrays are refused.
struct Sid([u8; SID_LEN]);

impl<'de> Deserialize<'de> for Sid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct SidVisitor;
        impl Visitor<'_> for SidVisitor {
            type Value = Sid;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 32-byte bin")
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Sid, E> {
                <[u8; SID_LEN]>::try_from(v)
                    .map(Sid)
                    .map_err(|_| E::invalid_length(v.len(), &self))
            }
        }
        d.deserialize_bytes(SidVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    ok: bool,
    sid: Sid,
    expires_at_ns: u64,
}

fn invalid_binding() -> ClientError {
    ClientError::Protocol("invalid token binding frame".into())
}

/// Strict decode of the first server frame: exactly `ok`, `sid`, `expires_at_ns`.
pub(crate) fn decode_binding(buf: &[u8]) -> Result<TokenBinding, ClientError> {
    if !matches!(buf.first(), Some(0x80..=0x8f | 0xde | 0xdf)) {
        return Err(invalid_binding());
    }
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(buf));
    let raw = RawBinding::deserialize(&mut decoder).map_err(|_| invalid_binding())?;
    if decoder.get_ref().position() != buf.len() as u64 {
        return Err(invalid_binding());
    }
    let sid = raw.sid.0;
    if !raw.ok || raw.expires_at_ns == 0 || sid.iter().all(|b| *b == 0) {
        return Err(invalid_binding());
    }
    Ok(TokenBinding {
        sid,
        expires_at_ns: raw.expires_at_ns,
    })
}

/// Reads the binding frame the server sends right after the upgrade.
/// Unbounded; the caller applies the handshake timeout.
pub(crate) async fn read_binding<S>(
    ws: &mut WebSocketStream<S>,
) -> Result<TokenBinding, ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(bytes))) => return decode_binding(&bytes),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(Message::Close(Some(frame)))) => {
                return Err(ClientError::ConnectionClosed(
                    WsPeerClose::new(u16::from(frame.code), &frame.reason).into_cause(),
                ));
            }
            Some(Ok(Message::Close(None))) | None => {
                return Err(ClientError::ConnectionClosed(CloseCause::PeerEof));
            }
            Some(Ok(Message::Text(_) | Message::Frame(_))) => {
                return Err(ClientError::Protocol(
                    "unexpected websocket message before binding".into(),
                ));
            }
            Some(Err(e)) => return Err(ClientError::handshake_with("read token binding", e)),
        }
    }
}
