//! Listener-profile abstraction with bind-time policy enforcement
//! (TRANSPORT_TCP §2.2 NORMATIVE).
//!
//! Spec §2.2 mandates: a server with `profile = "plain"` (no TLS) MUST
//! bind only to loopback addresses (`127.0.0.0/8`, `::1`) or Unix domain
//! sockets — never to public network interfaces. Without this gate an
//! operator misconfiguration could expose the SCRAM handshake unencrypted
//! on a public IP, defeating the channel-binding §4.2 requirement.
//!
//! [`bind_validated`] enforces this at server start. A non-loopback bind
//! attempt with `ListenerProfile::Plain` returns
//! [`ListenerBindError::PlainOnNonLoopback`] BEFORE the listener is
//! created, so a misconfigured server fails closed at boot rather than
//! silently accepting unencrypted public traffic.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use thiserror::Error;
use tokio::net::TcpListener;

/// Listener security profile per TRANSPORT_TCP §2.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerProfile {
    /// TLS 1.3 with channel binding via TLS exporter (`binding_mode = 0x01`).
    /// Default profile. Allowed on any address.
    TlsExporter,
    /// TLS 1.3 without exporter usage (e.g. browsers via WebSocket where
    /// the JS environment can't access the exporter — `binding_mode = 0x02`).
    /// Allowed on any address.
    TlsNoExport,
    /// Plain TCP (no TLS). Permitted ONLY on loopback addresses or Unix
    /// domain sockets. The bind validator refuses non-loopback addresses
    /// to fail closed on misconfiguration. Use case: same-host embedded
    /// scenarios where the protection model is process-isolation rather
    /// than transport encryption.
    Plain,
}

impl ListenerProfile {
    /// Whether this profile permits the given socket address.
    pub fn allows(&self, addr: &SocketAddr) -> bool {
        match self {
            ListenerProfile::TlsExporter | ListenerProfile::TlsNoExport => true,
            ListenerProfile::Plain => is_loopback(addr.ip()),
        }
    }
}

/// Per spec §2.2: `127.0.0.0/8` for IPv4 plus `::1` for IPv6 are the
/// canonical loopback ranges. We use the standard library's classifier
/// rather than re-implementing range checks.
fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Errors raised by [`bind_validated`].
#[derive(Debug, Error)]
pub enum ListenerBindError {
    /// Configuration violation: `Plain` profile attempted on a non-loopback
    /// address (would expose unencrypted SCRAM on the public network).
    #[error("plain TCP listener requires loopback bind address; refused {0}")]
    PlainOnNonLoopback(SocketAddr),
    /// Underlying tokio bind error.
    #[error("tcp bind: {0}")]
    Bind(#[from] std::io::Error),
}

/// Bind a [`TcpListener`] with profile-aware policy enforcement.
///
/// For `Plain`: refuses any non-loopback address with
/// [`ListenerBindError::PlainOnNonLoopback`] **before** the socket is
/// created. For TLS profiles: passes through to `TcpListener::bind`.
pub async fn bind_validated(
    addr: SocketAddr,
    profile: ListenerProfile,
) -> Result<TcpListener, ListenerBindError> {
    if !profile.allows(&addr) {
        return Err(ListenerBindError::PlainOnNonLoopback(addr));
    }
    let listener = TcpListener::bind(addr).await?;
    Ok(listener)
}

/// Pure validation predicate (no I/O). Returns `Err` if `addr` violates
/// the profile policy. Useful for static configuration validation in
/// tests + CLI tools.
pub fn validate_addr(addr: SocketAddr, profile: ListenerProfile) -> Result<(), ListenerBindError> {
    if !profile.allows(&addr) {
        return Err(ListenerBindError::PlainOnNonLoopback(addr));
    }
    Ok(())
}

/// Common loopback addresses for documentation / examples / tests.
#[allow(dead_code)]
pub const LOOPBACK_V4: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
#[allow(dead_code)]
pub const LOOPBACK_V6: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);
