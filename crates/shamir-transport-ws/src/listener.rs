//! WS listener-profile enforcement (TRANSPORT_WS, mirrors TCP §2.2).
//!
//! Spec TRANSPORT_WS mandates TLS 1.3 (wss://) for both native and browser
//! endpoints. Plain WS (ws://) carries no encryption AND no TLS exporter
//! → MITM relay reads session_id in plaintext after auth_ok and owns the
//! session. The protocol cannot defend against this without TLS or an
//! inner secure-channel layer (Noise NK; see `docs/roadmap/BROWSER_WASM_PLAN.md` for
//! v2 roadmap).
//!
//! [`WsListenerProfile`] enumerates the three valid combinations; plain
//! WS is allowed ONLY on loopback (analogous to plain TCP per
//! TRANSPORT_TCP §2.2). The bind validator refuses non-loopback +
//! plain to fail closed at server boot.

use std::net::{IpAddr, SocketAddr};
use thiserror::Error;
use tokio::net::TcpListener;

/// WebSocket listener security profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsListenerProfile {
    /// Native WSS (`binding_mode = 0x01`). TLS 1.3 + TLS exporter channel
    /// binding. Allowed on any address.
    Wss,
    /// Browser WSS (`binding_mode = 0x02`). TLS 1.3 but no exporter (the
    /// JS environment can't access it). Origin header validation required
    /// (handled by the accept path, not this module). Allowed on any
    /// address.
    WssBrowser,
    /// Plain WebSocket (`binding_mode = 0x00`). NO TLS — only permitted on
    /// loopback. Use case: same-host embedded scenarios where the
    /// protection model is process isolation. Equivalent to
    /// `ListenerProfile::Plain` in shamir-transport-tcp.
    PlainWsLoopback,
}

impl WsListenerProfile {
    /// Whether this profile permits the given socket address.
    pub fn allows(&self, addr: &SocketAddr) -> bool {
        match self {
            WsListenerProfile::Wss | WsListenerProfile::WssBrowser => true,
            WsListenerProfile::PlainWsLoopback => is_loopback(addr.ip()),
        }
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Errors raised by [`bind_validated`].
#[derive(Debug, Error)]
pub enum WsBindError {
    /// Plain WS attempted on a non-loopback address (would expose
    /// unencrypted SCRAM + session bearer-tokens on the network).
    #[error("plain ws listener requires loopback bind address; refused {0}")]
    PlainOnNonLoopback(SocketAddr),
    /// Underlying tokio bind error.
    #[error("tcp bind: {0}")]
    Bind(#[from] std::io::Error),
}

/// Bind a TCP listener (the WS upgrade happens later, after TLS terminate)
/// with profile-aware policy.
///
/// For `PlainWsLoopback`: refuses any non-loopback address with
/// [`WsBindError::PlainOnNonLoopback`] **before** the socket is created.
/// For `Wss` / `WssBrowser`: passes through.
pub async fn bind_validated(
    addr: SocketAddr,
    profile: WsListenerProfile,
) -> Result<TcpListener, WsBindError> {
    if !profile.allows(&addr) {
        return Err(WsBindError::PlainOnNonLoopback(addr));
    }
    Ok(TcpListener::bind(addr).await?)
}

/// Pure validation predicate (no I/O) — useful for static config checks.
pub fn validate_addr(addr: SocketAddr, profile: WsListenerProfile) -> Result<(), WsBindError> {
    if !profile.allows(&addr) {
        return Err(WsBindError::PlainOnNonLoopback(addr));
    }
    Ok(())
}
