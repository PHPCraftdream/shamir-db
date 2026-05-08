//! Wire envelope for post-handshake requests/responses.
//!
//! After `auth_ok`, every request the client sends carries the bearer
//! `session_id`. The server uses it to look up the [`Session`], runs the
//! per-request validity check (spec §7.5), then dispatches the request body
//! to the application.
//!
//! Wire format (msgpack, see TRANSPORT_TCP §6 / TRANSPORT_WS §5):
//!
//! ```text
//! Request:  { "sid": bytes(32), "rid": Optional<u32>, "req": <opaque> }
//! Response: { "rid": Optional<u32>, "res": <opaque> }
//! Error:    { "rid": Optional<u32>, "error": String }
//! ```

use crate::common::error::{Error, Result};
use crate::common::types::limits;
use serde::{Deserialize, Serialize};

/// Client → server request envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestEnvelope {
    /// Bearer session id from the most recent `auth_ok` / `resume_ok`.
    #[serde(with = "serde_bytes", rename = "sid")]
    pub session_id: Vec<u8>,
    /// Optional client-side correlation id.
    #[serde(rename = "rid", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u32>,
    /// Application-level request body — opaque msgpack value.
    #[serde(with = "serde_bytes")]
    pub req: Vec<u8>,
}

impl RequestEnvelope {
    /// Build with a fresh request id.
    pub fn new(session_id: [u8; limits::SESSION_ID_BYTES], request_id: Option<u32>, req: Vec<u8>) -> Self {
        Self {
            session_id: session_id.to_vec(),
            request_id,
            req,
        }
    }

    /// Encode to msgpack.
    pub fn to_msgpack(&self) -> Result<Vec<u8>> {
        rmp_serde::to_vec_named(self).map_err(|e| Error::Encoding(format!("envelope encode: {e}")))
    }

    /// Decode from msgpack bytes.
    pub fn from_msgpack(buf: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(buf).map_err(|e| Error::Encoding(format!("envelope decode: {e}")))
    }

    /// Return the session id as a fixed-size array, validating length.
    pub fn session_id_array(&self) -> Result<[u8; limits::SESSION_ID_BYTES]> {
        if self.session_id.len() != limits::SESSION_ID_BYTES {
            return Err(Error::InvalidInput("envelope: session_id wrong length"));
        }
        let mut out = [0u8; limits::SESSION_ID_BYTES];
        out.copy_from_slice(&self.session_id);
        Ok(out)
    }
}

/// Server → client response envelope (success path).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseEnvelope {
    /// Echoes `RequestEnvelope.request_id` if present.
    #[serde(rename = "rid", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u32>,
    /// Application-level response body.
    #[serde(with = "serde_bytes")]
    pub res: Vec<u8>,
}

impl ResponseEnvelope {
    /// Build a success response.
    pub fn ok(request_id: Option<u32>, res: Vec<u8>) -> Self {
        Self { request_id, res }
    }

    /// Encode to msgpack.
    pub fn to_msgpack(&self) -> Result<Vec<u8>> {
        rmp_serde::to_vec_named(self).map_err(|e| Error::Encoding(format!("response encode: {e}")))
    }

    /// Decode from msgpack.
    pub fn from_msgpack(buf: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(buf).map_err(|e| Error::Encoding(format!("response decode: {e}")))
    }
}

/// Server → client error envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    /// Echoes the request id if present.
    #[serde(rename = "rid", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u32>,
    /// Generic error string from spec §14 (e.g. "session_invalidated",
    /// "session_expired", "authentication_failed").
    pub error: String,
}

impl ErrorEnvelope {
    /// Build.
    pub fn new(request_id: Option<u32>, error: impl Into<String>) -> Self {
        Self {
            request_id,
            error: error.into(),
        }
    }

    /// Encode.
    pub fn to_msgpack(&self) -> Result<Vec<u8>> {
        rmp_serde::to_vec_named(self).map_err(|e| Error::Encoding(format!("error encode: {e}")))
    }

    /// Decode.
    pub fn from_msgpack(buf: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(buf).map_err(|e| Error::Encoding(format!("error decode: {e}")))
    }
}
