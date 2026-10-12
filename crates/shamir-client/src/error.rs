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

    #[error("transaction not known committed: {0:?}")]
    TransactionOutcome(Box<shamir_query_types::batch::TransactionInfo>),

    #[error("{message}")]
    AsOfVisibilityTimeout {
        code: String,
        message: String,
        requested: u64,
        visible_through: u64,
        blocking_version: u64,
    },
    // These wire errors carry only code/message, not structured positions.
    #[error("as_of_future_version: {message}")]
    AsOfFutureVersion { message: String },
    #[error("as_of_reopen_required: {message}")]
    AsOfReopenRequired { message: String },
    #[error("visibility_future_version: {message}")]
    VisibilityFutureVersion { message: String },
    #[error("visibility_wait_invalid: {message}")]
    VisibilityWaitInvalid { message: String },
    #[error("write committed; never resend: {message}")]
    WriteCommittedVisibilityPending { message: String },

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

    /// The connection was closed (EOF, I/O error, or explicit `close()`)
    /// before the response arrived.
    #[error("connection closed")]
    ConnectionClosed,

    /// The TCP connection to the server did not complete within the
    /// configured `ConnectOptions::connect_timeout` budget.
    #[error("connect timed out after {0:?}")]
    ConnectTimeout(std::time::Duration),

    /// A request was sent but no response arrived within the configured
    /// `ConnectOptions::request_timeout` budget.
    #[error("request timed out after {0:?}")]
    RequestTimeout(std::time::Duration),
}

impl From<rustls::Error> for ClientError {
    fn from(e: rustls::Error) -> Self {
        ClientError::Tls(e.to_string())
    }
}
