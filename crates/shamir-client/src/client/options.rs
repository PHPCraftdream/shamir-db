//! Connection option structs for the ShamirDB client.

use std::net::SocketAddr;

use zeroize::Zeroizing;

/// Connection parameters.
pub struct ConnectOptions {
    /// Server address (host:port).
    pub addr: SocketAddr,
    /// SNI hostname for TLS — usually the same hostname the server
    /// generated its self-signed certificate for (`localhost` in tests).
    pub server_name: String,
    /// Username (raw — will be NFC + UsernameCaseMapped normalised).
    pub username: String,
    /// Plaintext password. Zeroized after handshake completes.
    pub password: Zeroizing<Vec<u8>>,
    /// Trust-on-first-use: if `trusted_pin` is `None`, accept whatever
    /// Ed25519 public key the server presents on this first connection.
    /// Set to `false` once the pin is known and persisted.
    pub accept_new_host: bool,
    /// Pre-pinned `SHA256(server_ed25519_pub_key)`. When `Some`, the
    /// SDK validates against this hash and refuses on mismatch (spec
    /// `ServerIdentityChanged`). When `None`, requires
    /// `accept_new_host = true` and captures the pin during handshake;
    /// retrieve it via [`Client::server_pub_key_pin`] for persistence.
    pub trusted_pin: Option<[u8; 32]>,
    /// Deadline for establishing the underlying TCP connection
    /// ([`TcpStream::connect`]). `Some(d)` bounds the connect attempt and
    /// returns [`ClientError::ConnectTimeout`] if the socket does not
    /// connect within `d`. `None` (the default for existing callers)
    /// preserves the prior unbounded-wait behaviour — the OS TCP stack's
    /// own timeout is the only bound. Mirrors the TS SDK's
    /// `connectTimeoutMs`.
    pub connect_timeout: Option<std::time::Duration>,
    /// Deadline for a single request/response round-trip in
    /// [`Client::execute`]/[`Client::ping`]/etc. `Some(d)` bounds the
    /// response await and returns [`ClientError::RequestTimeout`] if no
    /// response arrives within `d`. `None` (the default) preserves the
    /// prior unbounded-wait behaviour. Mirrors the TS SDK's
    /// `requestTimeoutMs`.
    pub request_timeout: Option<std::time::Duration>,
}

/// Options for fast session resumption via a previously-issued ticket.
///
/// Obtain the ticket and `pinned_hash` from a prior [`Client::connect`] via
/// [`Client::resumption_ticket`] and [`Client::server_pub_key_pin`].
pub struct ResumeOptions {
    /// Server address (host:port).
    pub addr: SocketAddr,
    /// SNI hostname for TLS.
    pub server_name: String,
    /// Resumption ticket bytes obtained from a prior session.
    pub ticket: Vec<u8>,
    /// `SHA256(server_ed25519_pub_key)` pinned during the initial connection.
    /// The resumed session will carry the same pin.
    pub pinned_hash: [u8; 32],
}

/// Connection parameters for [`Client::connect_local`] — local IPC
/// transport (Unix domain socket / Windows Named Pipe), spec
/// TRANSPORT_UNIX.md. No TLS, no `server_name`/CA — the OS-level access
/// boundary (file permissions / DACL, enforced by `shamir-transport-ipc`
/// on the SERVER side) replaces transport encryption; the Ed25519 identity
/// TOFU-pin dance still applies exactly as it does for [`ConnectOptions`],
/// since that is layered on SCRAM, not on TLS.
pub struct ConnectLocalOptions {
    /// Local endpoint — a filesystem path (Unix) or a Named Pipe name
    /// (Windows, e.g. `\\.\pipe\shamir-db`). Callers don't branch on OS:
    /// `shamir_transport_ipc::connect` resolves the same string
    /// platform-appropriately.
    pub addr: String,
    /// Username (raw — will be NFC + UsernameCaseMapped normalised).
    pub username: String,
    /// Plaintext password. Zeroized after handshake completes.
    pub password: Zeroizing<Vec<u8>>,
    /// Trust-on-first-use — see [`ConnectOptions::accept_new_host`].
    pub accept_new_host: bool,
    /// Pre-pinned `SHA256(server_ed25519_pub_key)` — see
    /// [`ConnectOptions::trusted_pin`].
    pub trusted_pin: Option<[u8; 32]>,
    /// Deadline for establishing the underlying IPC connection — see
    /// [`ConnectOptions::connect_timeout`].
    pub connect_timeout: Option<std::time::Duration>,
    /// Deadline for a single request/response round-trip — see
    /// [`ConnectOptions::request_timeout`].
    pub request_timeout: Option<std::time::Duration>,
}
