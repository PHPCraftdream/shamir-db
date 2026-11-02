use crate::config::Endpoint;
use bytes::Bytes;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

/// Authentication header cardinality; bytes are never interpreted here.
#[derive(Clone, Copy, Debug)]
pub enum AuthHeader<'a> {
    /// The request has no Authorization header.
    Missing,
    /// The request has exactly one Authorization header.
    Single(&'a [u8]),
    /// The request has multiple Authorization headers.
    Multiple,
}

/// TCP peer information; forwarded-for values are raw and untrusted.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// Remote TCP peer address.
    pub addr: SocketAddr,
    /// Whether the connection uses TLS.
    pub tls: bool,
    /// Raw values of the forwarded-for headers.
    pub forwarded_for: Vec<Vec<u8>>,
}

/// Authentication decision from the application.
#[derive(Debug)]
pub enum GateDecision<G> {
    /// Authentication succeeded with this grant.
    Allow(G),
    /// Authentication failed or credentials are missing.
    Unauthorized,
    /// The authenticated caller is forbidden, with an application code.
    Forbidden(String),
    /// Authentication is rate-limited for the given retry interval.
    RateLimited {
        /// Seconds before a retry is appropriate.
        retry_after_secs: u64,
    },
    /// Authentication is temporarily unavailable for the given retry interval.
    Unavailable {
        /// Seconds before a retry is appropriate.
        retry_after_secs: u64,
    },
}

/// Application-provided authentication boundary.
pub trait TokenGate: Send + Sync + 'static {
    /// Grant type retained for an authenticated request.
    type Grant: Send + Sync + 'static;
    /// Native async trait methods preserve `Send` for spawned connection futures; this crate's MSRV is Rust 1.85+.
    fn authenticate<'a>(
        &'a self,
        auth: AuthHeader<'a>,
        endpoint: Endpoint,
        peer: PeerInfo,
    ) -> impl Future<Output = GateDecision<Self::Grant>> + Send + 'a;
    /// Reports whether an earlier grant remains valid.
    fn still_valid(&self, grant: &Self::Grant) -> bool;
}

/// REST execution result.
pub struct ExecOutcome {
    /// Application response status.
    pub status: ExecStatus,
    /// MessagePack response body.
    pub body: Bytes,
    /// Optional value held until response transmission completes.
    pub hold: Option<Box<dyn Send + Sync>>,
}

/// Application response status (kept deliberately limited).
#[derive(Clone, Copy, Debug)]
pub enum ExecStatus {
    /// Successful execution.
    Ok,
    /// The request was invalid.
    BadRequest,
    /// The operation is forbidden.
    Forbidden,
    /// The request is rate-limited.
    RateLimited,
    /// The service is unavailable.
    Unavailable,
}

/// Generic application executor; the method returns an explicit Send future.
pub trait RestExecutor<G>: Send + Sync + 'static {
    /// Executes a request body using its authenticated grant.
    fn execute<'a>(
        &'a self,
        grant: &'a G,
        body: Bytes,
    ) -> impl Future<Output = ExecOutcome> + Send + 'a;
}

/// Per-connection admission hook. A TCP dependency is also transitively needed by `shamir-transport-tcp` for its TLS material helpers and bind policy; there is no direct application dependency.
pub trait ConnAdmission: Send + Sync + 'static {
    /// Admits a peer and returns a permit held for the connection, or rejects it.
    fn admit(&self, peer: &SocketAddr) -> Option<Box<dyn Send + Sync>>;
}

/// Stable boxed future alias for transport internals.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
