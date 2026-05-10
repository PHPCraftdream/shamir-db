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

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(ip: &str, port: u16) -> SocketAddr {
        if ip.contains(':') {
            format!("[{}]:{}", ip, port).parse().unwrap()
        } else {
            format!("{}:{}", ip, port).parse().unwrap()
        }
    }

    #[test]
    fn wss_allows_any_address() {
        assert!(WsListenerProfile::Wss.allows(&sa("8.8.8.8", 7332)));
        assert!(WsListenerProfile::Wss.allows(&sa("0.0.0.0", 7332)));
        assert!(WsListenerProfile::Wss.allows(&sa("::", 7332)));
    }

    #[test]
    fn wss_browser_allows_any_address() {
        assert!(WsListenerProfile::WssBrowser.allows(&sa("8.8.8.8", 7333)));
        assert!(WsListenerProfile::WssBrowser.allows(&sa("0.0.0.0", 7333)));
    }

    #[test]
    fn plain_loopback_allows_127_0_0_1() {
        assert!(WsListenerProfile::PlainWsLoopback.allows(&sa("127.0.0.1", 7334)));
        assert!(WsListenerProfile::PlainWsLoopback.allows(&sa("127.255.255.254", 7334)));
        assert!(WsListenerProfile::PlainWsLoopback.allows(&sa("::1", 7334)));
    }

    #[test]
    fn plain_loopback_refuses_lan() {
        assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("192.168.1.5", 7334)));
        assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("8.8.8.8", 7334)));
    }

    #[test]
    fn plain_loopback_refuses_unspecified() {
        // 0.0.0.0 / :: bind to all interfaces — explicit guard.
        assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("0.0.0.0", 7334)));
        assert!(!WsListenerProfile::PlainWsLoopback.allows(&sa("::", 7334)));
    }

    #[test]
    fn validate_addr_rejects_plain_ws_on_lan() {
        let r = validate_addr(sa("192.168.1.5", 7334), WsListenerProfile::PlainWsLoopback);
        assert!(matches!(r, Err(WsBindError::PlainOnNonLoopback(_))));
    }

    #[test]
    fn validate_addr_passes_for_wss_on_lan() {
        assert!(validate_addr(sa("192.168.1.5", 7332), WsListenerProfile::Wss).is_ok());
    }

    #[tokio::test]
    async fn bind_validated_succeeds_for_plain_loopback() {
        let l = bind_validated(sa("127.0.0.1", 0), WsListenerProfile::PlainWsLoopback)
            .await
            .unwrap();
        assert!(l.local_addr().unwrap().ip().is_loopback());
    }

    #[tokio::test]
    async fn bind_validated_refuses_plain_on_unspecified() {
        let r = bind_validated(sa("0.0.0.0", 0), WsListenerProfile::PlainWsLoopback).await;
        assert!(matches!(r, Err(WsBindError::PlainOnNonLoopback(_))));
    }
}
