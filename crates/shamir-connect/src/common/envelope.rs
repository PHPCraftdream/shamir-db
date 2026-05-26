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
use std::convert::TryInto;

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
    pub fn new(
        session_id: [u8; limits::SESSION_ID_BYTES],
        request_id: Option<u32>,
        req: Vec<u8>,
    ) -> Self {
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

/// **Optim #9:** zero-copy borrowed envelope for the **client encode** path.
///
/// Symmetric to [`RequestEnvelopeView`] (which is for server decode):
/// `RequestEnvelopeRef<'a>` lets a client serialize a request without
/// allocating a `Vec<u8>` for `session_id`. Useful in tight client-side
/// request loops where the same `[u8; 32]` session id is sent on every
/// request.
///
/// ```rust,ignore
/// let sid: [u8; 32] = /* from auth_ok */;
/// let body = b"...";
/// let envelope = RequestEnvelopeRef {
///     session_id: &sid,
///     request_id: Some(42),
///     req: body,
/// };
/// let bytes = envelope.to_msgpack()?;
/// ```
///
/// Wire format identical to [`RequestEnvelope`] — verified by
/// `request_envelope_ref_wire_compat_with_owning` integration test.
#[derive(Debug, Serialize)]
pub struct RequestEnvelopeRef<'a> {
    /// Bearer session id (always 32 bytes).
    #[serde(with = "serde_bytes", rename = "sid")]
    pub session_id: &'a [u8; 32],
    /// Optional client-side correlation id.
    #[serde(rename = "rid", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u32>,
    /// Application-level request body — borrowed.
    #[serde(with = "serde_bytes")]
    pub req: &'a [u8],
}

impl<'a> RequestEnvelopeRef<'a> {
    /// Encode to msgpack — single allocation for the output Vec; no per-call
    /// copy of `session_id` or `req`.
    pub fn to_msgpack(&self) -> Result<Vec<u8>> {
        rmp_serde::to_vec_named(self)
            .map_err(|e| Error::Encoding(format!("envelope ref encode: {e}")))
    }
}

/// **Optim #4:** zero-copy borrowed view over a request envelope.
///
/// Use this on the server-side hot path instead of the owning
/// [`RequestEnvelope`]: `session_id` and `req` are `&[u8]` borrows that
/// alias directly into the input buffer — no allocation, no copy.
///
/// Build via [`RequestEnvelopeView::from_msgpack`] which is the borrowed
/// counterpart of [`RequestEnvelope::from_msgpack`].
#[derive(Debug, Deserialize)]
pub struct RequestEnvelopeView<'a> {
    /// Bearer session id (must be exactly 32 bytes — validated at access time).
    #[serde(borrow, with = "serde_bytes", rename = "sid")]
    pub session_id: &'a [u8],
    /// Optional client-side correlation id.
    #[serde(rename = "rid", default)]
    pub request_id: Option<u32>,
    /// Application-level request body — opaque borrowed slice.
    #[serde(borrow, with = "serde_bytes")]
    pub req: &'a [u8],
}

impl<'a> RequestEnvelopeView<'a> {
    /// Decode from msgpack bytes WITHOUT copying.
    ///
    /// The returned view borrows from `buf`; `buf` must outlive the view.
    pub fn from_msgpack(buf: &'a [u8]) -> Result<Self> {
        rmp_serde::from_slice(buf)
            .map_err(|e| Error::Encoding(format!("envelope view decode: {e}")))
    }

    /// Validate `session_id.len() == 32` and return a borrowed array reference.
    ///
    /// Zero-copy: returns a `&[u8; 32]` that points to the same memory as
    /// the underlying buffer.
    pub fn session_id_array(&self) -> Result<&[u8; limits::SESSION_ID_BYTES]> {
        self.session_id
            .try_into()
            .map_err(|_| Error::InvalidInput("envelope: session_id wrong length"))
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
