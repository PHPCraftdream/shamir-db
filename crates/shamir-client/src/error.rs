use std::sync::Arc;

use thiserror::Error;

/// All errors surfaced by the SDK.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("tls: {context}: {source}")]
    Tls {
        /// What failed (`client config`, `server name`, `rustls`).
        context: &'static str,
        /// Typed TLS-layer source (rustls / pki-types / config error).
        /// `Box<dyn Error>` is the sanctioned boundary last resort here: the
        /// client boundary aggregates TLS sources of several distinct
        /// concrete types (rustls::Error, InvalidDnsNameError,
        /// TlsConfigError), none of which this crate owns.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("transport: {context}: {source}")]
    Transport {
        /// Wire phase that failed (`send auth_init`, `read challenge`, …).
        context: &'static str,
        /// The real framing/IO failure (`FrameError::Io` wraps the typed
        /// `std::io::Error` one hop further down the chain).
        #[source]
        source: shamir_transport_tcp::framing::FrameError,
    },

    #[error("handshake: {context}")]
    Handshake {
        /// Failure description (SCRAM stage, exporter availability, …).
        context: String,
        /// Typed SCRAM/KDF source when one exists (`model::Error`,
        /// `JoinError`). `Box<dyn Error>` is the sanctioned boundary last
        /// resort: the handshake mixes sources of unrelated concrete types
        /// (shamir-connect `model::Error`, tokio `JoinError`) that this
        /// crate does not own.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Server replied but the response shape is not what we expected
    /// (e.g. `Pong` for an `Execute`, or msgpack decode failed).
    #[error("protocol: {0}")]
    Protocol(String),

    /// Server replied with an envelope-level error. The envelope carries a
    /// single string which IS the spec §14 code (e.g. `session_expired`,
    /// `session_invalidated`, `rate_limited`) — there is no separate
    /// message field to surface.
    #[error("server error [{code}]")]
    Server { code: String },

    /// Server returned a typed `DbResponse::Error`.
    #[error("db error [{code}]: {message}")]
    Db { code: String, message: String },

    #[error("server identity rotation pending: transition until {transition_until_ns}")]
    IdentityRotationPending {
        previous_pin: [u8; 32],
        new_pin: [u8; 32],
        transition_until_ns: u64,
    },

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
    #[error("resync_required: {message}")]
    ResyncRequired { message: String },
    #[error("visibility_future_version: {message}")]
    VisibilityFutureVersion { message: String },
    #[error("visibility_wait_invalid: {message}")]
    VisibilityWaitInvalid { message: String },
    #[error("write committed; never resend: {message}")]
    WriteCommittedVisibilityPending { message: String },

    /// Server returned a request_id that doesn't match what we sent.
    #[error("request_id mismatch: sent {sent:?}, got {got:?}")]
    RequestIdMismatch { sent: Option<u32>, got: Option<u32> },

    /// The caller explicitly set `result_encoding = Id` on a batch carrying
    /// `$query` / `$param` refs (or a sub-batch). Under Id encoding the
    /// referenced intermediate becomes an opaque `QueryRecord::IdBytes`
    /// whose `as_value()` is `Null`, so ref path-resolution silently
    /// resolves to `Null` instead of the earlier alias's result. The
    /// request is rejected before it is sent.
    #[error("result_encoding=Id on ref-bearing batch (alias '{alias}'): the referenced intermediate would become an opaque IdBytes whose as_value() is Null")]
    IdEncodingWithRefs {
        /// Alias of the FIRST ref-bearing query in the batch.
        alias: String,
    },

    #[error("encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    /// Username failed normalisation (NFC + UsernameCaseMapped).
    #[error("invalid username: {0}")]
    InvalidUsername(String),

    /// The connection was closed (EOF, I/O error, explicit `close()`, client
    /// drop, or reader-task cancellation) before the response arrived. The
    /// first cause observed by [`CloseCause`]-producing shutdown wins.
    #[error("connection closed ({0})")]
    ConnectionClosed(CloseCause),

    /// The TCP connection to the server did not complete within the
    /// configured `ConnectOptions::connect_timeout` budget.
    #[error("connect timed out after {0:?}")]
    ConnectTimeout(std::time::Duration),

    /// A request was sent but no response arrived within the configured
    /// `ConnectOptions::request_timeout` budget.
    #[error("request timed out after {0:?}")]
    RequestTimeout(std::time::Duration),

    /// The handshake (or its DNS stage) did not complete within the
    /// configured `ConnectOptions::handshake_timeout` budget. The phase
    /// string is the stage the handshake was actually in when the deadline
    /// elapsed (`dns`, `tcp`, `tls`, `scram`, `kdf`) — never guessed.
    #[error("handshake timed out after {0:?} (phase: {1})")]
    HandshakeTimeout(std::time::Duration, std::sync::Arc<str>),
}

/// Why the connection reached its terminal closed state. Shared by every
/// waiter and registry after shutdown; the first cause recorded wins.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseCause {
    /// Server closed its write side / peer EOF.
    PeerEof,
    /// Read error from the transport (formatted `std::io::Error`).
    Io(Arc<str>),
    /// Explicit [`Client::close()`](crate::Client::close).
    LocalClose,
    /// The [`Client`](crate::Client) was dropped without an explicit close.
    ClientDropped,
    /// The background reader task was aborted/cancelled before its loop ran
    /// to completion.
    ReaderCancelled,
}

impl std::fmt::Display for CloseCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloseCause::PeerEof => write!(f, "peer eof"),
            CloseCause::Io(e) => write!(f, "io: {e}"),
            CloseCause::LocalClose => write!(f, "local close"),
            CloseCause::ClientDropped => write!(f, "client dropped"),
            CloseCause::ReaderCancelled => write!(f, "reader cancelled"),
        }
    }
}

impl From<rustls::Error> for ClientError {
    fn from(e: rustls::Error) -> Self {
        ClientError::Tls {
            context: "rustls",
            source: Box::new(e),
        }
    }
}

impl ClientError {
    /// TLS failure with a typed source (rustls, pki-types, config build).
    pub(crate) fn tls(
        context: &'static str,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        ClientError::Tls {
            context,
            source: source.into(),
        }
    }

    /// Wire send/read failure; the typed `std::io::Error` source survives.
    pub(crate) fn transport(
        context: &'static str,
        source: shamir_transport_tcp::framing::FrameError,
    ) -> Self {
        ClientError::Transport { context, source }
    }

    /// Handshake failure carrying no typed source (plain diagnostic).
    pub(crate) fn handshake(context: impl Into<String>) -> Self {
        ClientError::Handshake {
            context: context.into(),
            source: None,
        }
    }

    /// Handshake failure carrying a typed source (SCRAM `model::Error`,
    /// tokio `JoinError`, …); the source joins the error chain.
    pub(crate) fn handshake_with(
        context: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        let source = source.into();
        let detail = source.to_string();
        let context = context.into();
        let context = if context == detail {
            context
        } else {
            format!("{context}: {detail}")
        };
        ClientError::Handshake {
            context,
            source: Some(source),
        }
    }
}

/// Map a [`shamir_query_builder::batch::BuildError`] into a [`ClientError`] — the
/// single conversion used by every call site that turns a builder `Batch`
/// into a sent request via `try_build` (batch validation must surface, never
/// silently drop deferred registration errors).
pub(crate) fn client_batch_error(e: shamir_query_builder::batch::BuildError) -> ClientError {
    ClientError::Protocol(format!("batch validation: {e}"))
}
