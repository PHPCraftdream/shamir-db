use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Parameters of `ShamirClient.connectLocal`: SCRAM over a local IPC
/// endpoint.
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `password` is zeroized.
#[napi(object)]
pub struct ConnectLocalOptions {
    /// Unix socket path or `\\.\pipe\name` Windows pipe, used as is.
    pub addr: String,
    /// Username (raw — server-side normalisation applies).
    pub username: String,
    /// Plaintext password. Only the native copy is zeroized; the JavaScript
    /// string cannot be wiped.
    pub password: String,
    /// Trust-on-first-use, see `ConnectOptions.acceptNewHost`.
    pub accept_new_host: Option<bool>,
    /// Pre-pinned server identity hash, exactly 32 bytes.
    pub trusted_pin: Option<Buffer>,
    /// Deadline (ms) for the IPC connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for the whole handshake.
    pub handshake_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip.
    pub request_timeout_ms: Option<u32>,
}
