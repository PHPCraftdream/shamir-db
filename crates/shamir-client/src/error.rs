use thiserror::Error;

/// All errors surfaced by the SDK.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("tls: {0}")]
    Tls(String),

    #[error("transport: {0}")]
    Transport(String),

    #[error("handshake: {0}")]
    Handshake(String),

    /// Server replied but the response shape is not what we expected
    /// (e.g. `Pong` for an `Execute`, or msgpack decode failed).
    #[error("protocol: {0}")]
    Protocol(String),

    /// Server returned a typed `DbResponse::Error`.
    #[error("db error [{code}]: {message}")]
    Db { code: String, message: String },

    /// Server returned a request_id that doesn't match what we sent.
    #[error("request_id mismatch: sent {sent:?}, got {got:?}")]
    RequestIdMismatch { sent: Option<u32>, got: Option<u32> },

    #[error("encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    /// Username failed normalisation (NFC + UsernameCaseMapped).
    #[error("invalid username: {0}")]
    InvalidUsername(String),
}

impl From<rustls::Error> for ClientError {
    fn from(e: rustls::Error) -> Self {
        ClientError::Tls(e.to_string())
    }
}
