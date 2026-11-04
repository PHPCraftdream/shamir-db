//! Connection option structs for the ShamirDB client.

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use zeroize::Zeroizing;

use crate::client::transport::token::TokenTrust;
use crate::error::ClientError;

/// Connection parameters.
#[derive(Clone)]
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
    /// Deadline for the WHOLE TCP→TLS→SCRAM handshake ([`Client::connect`]).
    /// `Some(d)` bounds the TLS handshake and the SCRAM frame awaits —
    /// previously only the TCP connect was bounded, leaving the TLS and
    /// SCRAM awaits unbounded (RV-CLIENT-NODE-1CFB3796). Elapsing returns
    /// [`ClientError::HandshakeTimeout`]. `None` (the default for existing
    /// callers) preserves the prior unbounded behaviour.
    pub handshake_timeout: Option<std::time::Duration>,
    /// Deadline for a single request/response round-trip in
    /// [`Client::execute`]/[`Client::ping`]/etc. `Some(d)` bounds the
    /// response await and returns [`ClientError::RequestTimeout`] if no
    /// response arrives within `d`. `None` (the default) preserves the
    /// prior unbounded-wait behaviour. Mirrors the TS SDK's
    /// `requestTimeoutMs`.
    pub request_timeout: Option<std::time::Duration>,
}

/// Race `fut` against an ABSOLUTE deadline (RV-CLIENT-NODE-1CFB3796 request
/// deadlines): `None` runs `fut` unbounded; `Some(dl)` returns `Ok(None)`
/// when the deadline elapses first — the CALLER maps that to its own typed
/// timeout error. `Ok(Some(v))` is `fut`'s value; errors from `fut` pass
/// through unchanged. Deterministic under `tokio::time::pause()`.
pub async fn bound_until<T, F>(
    deadline: Option<tokio::time::Instant>,
    fut: F,
) -> Result<Option<T>, ClientError>
where
    // NOTE: `T` is the future's WHOLE output — callers pass futures with
    // `Output = Result<V, ClientError>`, so the success arm carries the
    // inner `Result` and the caller destructures `Ok(Some(Ok(v)))` /
    // `Ok(Some(Err(e)))` (errors pass through inside `Some`, unchanged).
    F: std::future::Future<Output = T>,
{
    match deadline {
        None => Ok(Some(fut.await)),
        Some(dl) => Ok(tokio::time::timeout_at(dl, fut).await.ok()),
    }
}

/// Resolve a `host:port` pair into a [`SocketAddr`] for
/// [`ConnectOptions::addr`] (RV-CLIENT-NODE-8FB7D9BF): IP literals (IPv4,
/// bare or bracketed IPv6) are used as-is; any other hostname is resolved
/// through the system resolver and the first address is returned.
pub async fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, ClientError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let candidate = format!("{host}:{port}");
    let mut addrs = tokio::net::lookup_host(&candidate).await?;
    addrs.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("host resolved to no addresses: {candidate}"),
        )
        .into()
    })
}

/// Options for fast session resumption via a previously-issued ticket.
///
/// Obtain the ticket and `pinned_hash` from a prior [`Client::connect`] via
/// [`Client::resumption_ticket`] and [`Client::server_pub_key_pin`].
///
/// The ticket is an ordinary `Vec`: retaining these options, or dropping the
/// future returned by `Client::resume` before its first poll, does not wipe it.
/// Prefer [`SecureResumeOptions`] for retained or cancellable options. Rust-owned
/// protection does not erase copies held by JavaScript or other foreign callers.
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

/// Resumption options whose Rust-owned ticket is wiped on drop, including
/// when retained or when an unpolled resume future is cancelled. This does not
/// wipe copies owned by JavaScript or other foreign callers.
pub struct SecureResumeOptions {
    /// Server address (host:port).
    pub addr: SocketAddr,
    /// SNI hostname for TLS.
    pub server_name: String,
    /// Resumption ticket, wiped when its owner is dropped.
    pub ticket: Zeroizing<Vec<u8>>,
    /// Pinned SHA256 of the server identity key.
    pub pinned_hash: [u8; 32],
}

/// Resumption options for [`Client::resume_local`] (local IPC: Unix domain
/// socket / Windows Named Pipe, binding mode 0x00). The ticket is wiped on
/// drop; there is no TLS and no server name.
pub struct ResumeLocalOptions {
    /// Raw platform endpoint, as in [`ConnectLocalOptions::addr`].
    pub addr: String,
    /// Resumption ticket, wiped when its owner is dropped.
    pub ticket: Zeroizing<Vec<u8>>,
    /// Pinned SHA256 of the server identity key; the resumed session carries it.
    pub pinned_hash: [u8; 32],
    /// Deadline for establishing the IPC connection (`None` = unbounded).
    pub connect_timeout: Option<std::time::Duration>,
}

impl From<ResumeOptions> for SecureResumeOptions {
    fn from(opts: ResumeOptions) -> Self {
        Self {
            addr: opts.addr,
            server_name: opts.server_name,
            ticket: Zeroizing::new(opts.ticket),
            pinned_hash: opts.pinned_hash,
        }
    }
}

/// Connection parameters for [`Client::connect_local`] — local IPC
/// transport (Unix domain socket / Windows Named Pipe), spec
/// TRANSPORT_UNIX.md. No TLS, no `server_name`/CA — the OS-level access
/// boundary (file permissions / DACL, enforced by `shamir-transport-ipc`
/// on the SERVER side) replaces transport encryption; the Ed25519 identity
/// TOFU-pin dance still applies exactly as it does for [`ConnectOptions`],
/// since that is layered on SCRAM, not on TLS.
pub struct ConnectLocalOptions {
    /// Raw platform endpoint: an absolute Unix socket path or a `\\.\pipe\name` Windows pipe. Validity is platform-specific; no logical-name resolution or prefixing occurs.
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

/// Options for
/// [`Client::connect_ws_browser`](crate::Client::connect_ws_browser)
/// — a SCRAM session over the browser-profile WebSocket listener (`kind: ws`,
/// `profile: tls_no_export`, path `/shamir/v1/browser`, binding mode 0x02,
/// `Origin` required). Built for TLS-terminating proxies that break the TLS
/// exporter: the TLS server is authenticated by the ordinary certificate
/// check against `trust` (WebPKI roots by default, caller-supplied anchors
/// when given); the Ed25519 identity TOFU-pin handling of [`ConnectOptions`]
/// is unchanged and still applies on top.
#[non_exhaustive]
pub struct BrowserWsOptions {
    /// Session options (address, credentials, pins, timeouts). `server_name`
    /// is the TLS SNI / hostname the certificate is checked against.
    pub inner: ConnectOptions,
    /// `Origin` header value sent on the WebSocket upgrade. The listener
    /// rejects the upgrade without an allowed `Origin`; an empty string means
    /// the header is omitted entirely (and is refused by the listener).
    pub origin: String,
    /// Which certificate authorities the TLS connection trusts.
    pub trust: TokenTrust,
}

impl BrowserWsOptions {
    pub fn new(inner: ConnectOptions, origin: impl Into<String>, trust: TokenTrust) -> Self {
        Self {
            inner,
            origin: origin.into(),
            trust,
        }
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = origin.into();
        self
    }

    pub fn with_trust(mut self, trust: TokenTrust) -> Self {
        self.trust = trust;
        self
    }
}

impl fmt::Debug for BrowserWsOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `inner` carries the password: it is never delegated to `Debug`.
        f.debug_struct("BrowserWsOptions")
            .field("addr", &self.inner.addr)
            .field("server_name", &self.inner.server_name)
            .field("username", &self.inner.username)
            .field("origin", &self.origin)
            .field("trust", &self.trust)
            .finish_non_exhaustive()
    }
}

/// Options for
/// [`Client::resume_ws_browser`](crate::Client::resume_ws_browser)
/// — ticket resumption over the browser-profile WebSocket listener (binding
/// mode 0x02). There is no `resume_proof` preamble on this binding (the
/// profile has no TLS exporter): the server is authenticated by the TLS
/// certificate check against `trust`, and the pinned hash rides unchanged on
/// the resumed session.
#[non_exhaustive]
pub struct BrowserResumeOptions {
    /// Resumption options (address, SNI, ticket, pinned hash). The ticket is
    /// an ordinary `Vec` until the future is polled — see [`ResumeOptions`].
    pub inner: ResumeOptions,
    /// `Origin` header value sent on the WebSocket upgrade (empty = omitted).
    pub origin: String,
    /// Which certificate authorities the TLS connection trusts.
    pub trust: TokenTrust,
}

impl BrowserResumeOptions {
    pub fn new(inner: ResumeOptions, origin: impl Into<String>, trust: TokenTrust) -> Self {
        Self {
            inner,
            origin: origin.into(),
            trust,
        }
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = origin.into();
        self
    }

    pub fn with_trust(mut self, trust: TokenTrust) -> Self {
        self.trust = trust;
        self
    }
}

impl fmt::Debug for BrowserResumeOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `inner` carries the bearer ticket: it never appears in `Debug`.
        f.debug_struct("BrowserResumeOptions")
            .field("addr", &self.inner.addr)
            .field("server_name", &self.inner.server_name)
            .field("origin", &self.origin)
            .field("trust", &self.trust)
            .finish_non_exhaustive()
    }
}
