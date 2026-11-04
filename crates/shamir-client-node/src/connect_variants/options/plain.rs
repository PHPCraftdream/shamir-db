use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Parameters of `ShamirClient.connectPlain`: SCRAM over plain TCP. The core
/// refuses any non-loopback address.
///
/// A JavaScript string cannot be wiped; only the owned native copy of
/// `password` is zeroized.
#[napi(object)]
pub struct ConnectPlainOptions {
    /// Server host; only its first resolved address is used.
    pub host: String,
    /// Server port.
    pub port: u32,
    /// Username (raw — server-side normalisation applies).
    pub username: String,
    /// Plaintext password. Only the native copy is zeroized; the JavaScript
    /// string cannot be wiped.
    pub password: String,
    /// Trust-on-first-use, see `ConnectOptions.acceptNewHost`.
    pub accept_new_host: Option<bool>,
    /// Pre-pinned server identity hash, exactly 32 bytes.
    pub trusted_pin: Option<Buffer>,
    /// Deadline (ms) for the TCP connect attempt.
    pub connect_timeout_ms: Option<u32>,
    /// Deadline (ms) for the whole handshake.
    pub handshake_timeout_ms: Option<u32>,
    /// Deadline (ms) for each request/response round-trip.
    pub request_timeout_ms: Option<u32>,
}
