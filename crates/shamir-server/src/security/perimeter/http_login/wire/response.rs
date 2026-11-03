use super::WireError;
use crate::security::tokens::TokenView;
use axum::http::StatusCode;
use bytes::Bytes;
use shamir_connect::server::token::{TokenChannels, TokenMode};

/// Values for the exact twelve-key BEGIN challenge.
pub struct Challenge<'a> {
    /// Listener security class.
    pub security: u8,
    /// Stable logical listener id.
    pub listener_id: &'a [u8; 16],
    /// Generated handshake id.
    pub handshake_id: &'a [u8; 32],
    /// SCRAM salt.
    pub salt: &'a [u8; 16],
    /// Argon2 memory in KiB.
    pub memory_kb: u32,
    /// Argon2 iterations.
    pub time: u32,
    /// Argon2 parallelism.
    pub parallelism: u32,
    /// Generated server nonce.
    pub server_nonce: &'a [u8; 32],
}

/// Values for the exact eleven-key FINISH success response.
pub struct Success<'a> {
    /// Server SCRAM signature.
    pub server_signature: &'a [u8; 32],
    /// One-time bearer token (never formatted in diagnostics).
    pub token: &'a str,
    /// Issued token record view.
    pub view: &'a TokenView,
    /// Canonical owner username.
    pub owner: &'a str,
}

/// Encode the exact challenge map using a bounded handwritten encoder.
pub fn challenge(value: &Challenge<'_>) -> Result<Bytes, WireError> {
    if value.security > 2 {
        return Err(WireError::Invalid);
    }
    let mut out = Vec::with_capacity(256);
    out.push(0x8c);
    key(&mut out, "protocol_version")?;
    out.push(1);
    key(&mut out, "binding_mode")?;
    out.push(3);
    key(&mut out, "security")?;
    out.push(value.security);
    key(&mut out, "listener_id")?;
    binary(&mut out, value.listener_id)?;
    key(&mut out, "handshake_id")?;
    binary(&mut out, value.handshake_id)?;
    key(&mut out, "salt")?;
    binary(&mut out, value.salt)?;
    key(&mut out, "kdf")?;
    string(&mut out, "argon2id")?;
    key(&mut out, "memory_kb")?;
    unsigned(&mut out, u64::from(value.memory_kb));
    key(&mut out, "time")?;
    unsigned(&mut out, u64::from(value.time));
    key(&mut out, "parallelism")?;
    unsigned(&mut out, u64::from(value.parallelism));
    key(&mut out, "argon2_version")?;
    out.push(0x13);
    key(&mut out, "server_nonce")?;
    binary(&mut out, value.server_nonce)?;
    bounded(out)
}

/// Encode the exact success response map.
pub fn success(value: &Success<'_>) -> Result<Bytes, WireError> {
    let mut out = Vec::with_capacity(4096);
    out.push(0x8b);
    key(&mut out, "protocol_version")?;
    out.push(1);
    key(&mut out, "server_signature")?;
    binary(&mut out, value.server_signature)?;
    key(&mut out, "token")?;
    string(&mut out, value.token)?;
    key(&mut out, "id")?;
    binary(&mut out, &value.view.id)?;
    key(&mut out, "label")?;
    string(&mut out, &value.view.label)?;
    key(&mut out, "owner")?;
    string(&mut out, value.owner)?;
    key(&mut out, "mode")?;
    string(&mut out, mode(value.view.mode))?;
    key(&mut out, "channels")?;
    string(&mut out, channels(value.view.channels))?;
    key(&mut out, "databases")?;
    match &value.view.databases {
        None => out.push(0xc0),
        Some(databases) => {
            if databases.is_empty() || databases.len() > 4 {
                return Err(WireError::Invalid);
            }
            out.push(0x90 | u8::try_from(databases.len()).map_err(|_| WireError::Invalid)?);
            for database in databases {
                string(&mut out, database)?;
            }
        }
    }
    key(&mut out, "created_at_ns")?;
    unsigned(&mut out, value.view.created_at_ns);
    key(&mut out, "expires_at_ns")?;
    unsigned(&mut out, value.view.expires_at_ns);
    bounded(out)
}

/// Encode an exact protocol error body, including retry only for 429/503.
pub fn error(code: &str, retry: Option<u32>) -> Result<Bytes, WireError> {
    let mut out = Vec::with_capacity(64);
    out.push(if retry.is_some() { 0x82 } else { 0x81 });
    key(&mut out, "error")?;
    string(&mut out, code)?;
    if let Some(seconds) = retry {
        if !(1..=3600).contains(&seconds) {
            return Err(WireError::Invalid);
        }
        key(&mut out, "retry_after")?;
        unsigned(&mut out, u64::from(seconds));
    }
    bounded(out)
}

/// Construct an opaque transport response with matching status and retry metadata.
pub fn response(
    status: u16,
    code: &str,
    retry: Option<u32>,
) -> Result<shamir_transport_http::LoginResponse, WireError> {
    let status = StatusCode::from_u16(status).map_err(|_| WireError::Invalid)?;
    let retry_status =
        status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE;
    if retry_status != retry.is_some() {
        return Err(WireError::Invalid);
    }
    if retry.is_some_and(|seconds| !(1..=3600).contains(&seconds)) {
        return Err(WireError::Invalid);
    }
    let body = error(code, retry)?;
    Ok(if let Some(seconds) = retry {
        shamir_transport_http::LoginResponse::with_retry_after(status, body, seconds)
    } else {
        shamir_transport_http::LoginResponse::new(status, body)
    })
}

fn key(out: &mut Vec<u8>, value: &str) -> Result<(), WireError> {
    string(out, value)
}
fn bounded(out: Vec<u8>) -> Result<Bytes, WireError> {
    if out.len() > 4096 {
        return Err(WireError::Invalid);
    }
    Ok(Bytes::from(out))
}
fn string(out: &mut Vec<u8>, text: &str) -> Result<(), WireError> {
    let bytes = text.as_bytes();
    if bytes.len() > 4096 {
        return Err(WireError::Invalid);
    }
    if bytes.len() < 32 {
        out.push(0xa0 | u8::try_from(bytes.len()).map_err(|_| WireError::Invalid)?);
    } else if bytes.len() <= 255 {
        out.extend_from_slice(&[
            0xd9,
            u8::try_from(bytes.len()).map_err(|_| WireError::Invalid)?,
        ]);
    } else {
        out.push(0xda);
        out.extend_from_slice(
            &u16::try_from(bytes.len())
                .map_err(|_| WireError::Invalid)?
                .to_be_bytes(),
        );
    }
    out.extend_from_slice(bytes);
    Ok(())
}
fn binary(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), WireError> {
    if bytes.len() > 255 {
        return Err(WireError::Invalid);
    }
    out.extend_from_slice(&[
        0xc4,
        u8::try_from(bytes.len()).map_err(|_| WireError::Invalid)?,
    ]);
    out.extend_from_slice(bytes);
    Ok(())
}
fn unsigned(out: &mut Vec<u8>, value: u64) {
    if value < 128 {
        out.push(value as u8);
    } else if value <= u64::from(u8::MAX) {
        out.extend_from_slice(&[0xcc, value as u8]);
    } else if value <= u64::from(u16::MAX) {
        out.push(0xcd);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value <= u64::from(u32::MAX) {
        out.push(0xce);
        out.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        out.push(0xcf);
        out.extend_from_slice(&value.to_be_bytes());
    }
}
fn mode(value: TokenMode) -> &'static str {
    match value {
        TokenMode::ReadOnly => "read_only",
        TokenMode::ReadWrite => "read_write",
    }
}
fn channels(value: TokenChannels) -> &'static str {
    match value {
        TokenChannels::Rest => "rest",
        TokenChannels::Ws => "ws",
        TokenChannels::Both => "both",
    }
}
