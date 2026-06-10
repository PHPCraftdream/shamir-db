//! Server-side WebSocket accept paths.
//!
//! Two endpoints per spec TRANSPORT_WS §2:
//!
//! - **`/shamir/v1`** (native): TLS 1.3 + WebSocket upgrade. Caller
//!   extracts the exporter via [`crate::tls_exporter::extract_tls_exporter_from_stream`]
//!   AFTER the WS handshake completes.
//!
//! - **`/shamir/v1/browser`** (browser): TLS 1.3 + WebSocket upgrade +
//!   mandatory `Origin` header validation. The exporter is NOT used;
//!   `binding_mode = 0x02` and `tls_exporter_or_zeros = [0u8; 32]`.

use crate::browser::{validate_origin, BrowserOriginPolicy, OriginRejected};
use crate::framing::MAX_WS_FRAME_SIZE;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::WebSocketStream;

/// Build the [`WebSocketConfig`] applied to every accepted server stream.
///
/// **NEW-1 (pre-auth buffering cap).** Tungstenite's defaults are
/// `max_message_size = 64 MiB` and `max_frame_size = 16 MiB`. Without an
/// explicit config an *unauthenticated* WSS peer can force the server to
/// buffer a whole 64 MiB `Message::Binary` before the framing layer's 4 KiB
/// pre-auth logical check (HIGH-1, enforced in [`crate::framing::ws_recv_into`])
/// ever runs — 10 000 such peers ≈ 640 GiB. We pin both ceilings to the
/// spec §8 `MAX_FRAME_SIZE_DATA` value (16 MiB) so the per-connection
/// buffering ceiling drops 4× (64 → 16 MiB) and a single WS message can
/// never exceed one frame.
///
/// **Residual.** This config is fixed at accept time — there is no
/// per-phase hook on the accept call to apply a *tighter* (e.g. 4 KiB) cap
/// during the handshake only. So an unauthenticated peer can still buffer up
/// to 16 MiB before the framing layer's 4 KiB logical reject fires. 16 MiB is
/// the practical floor for this transport; the logical 4 KiB pre-auth budget
/// (HIGH-1) still rejects oversized handshake frames immediately after
/// tungstenite hands them up. A future hardening could accept with a 4 KiB
/// cap and use tungstenite's `set_config` to relax it to 16 MiB only *after*
/// `auth_ok`, but that is out of scope here.
pub(crate) fn server_ws_config() -> WebSocketConfig {
    // Struct-update from `Default` so the deprecated `max_send_queue` field
    // (and any future additions) keep their default without tripping the
    // `deprecated` lint under `-D warnings`.
    WebSocketConfig {
        max_message_size: Some(MAX_WS_FRAME_SIZE),
        max_frame_size: Some(MAX_WS_FRAME_SIZE),
        ..Default::default()
    }
}

/// Errors raised when accepting a WS upgrade.
#[derive(Debug, Error)]
pub enum WsAcceptError {
    /// Underlying tungstenite handshake error.
    #[error("ws handshake: {0}")]
    Handshake(#[from] tokio_tungstenite::tungstenite::Error),
    /// Origin policy rejected the upgrade (browser endpoint only).
    #[error("origin: {0}")]
    OriginRejected(#[from] OriginRejected),
    /// Wrong path — caller routed to the wrong accept fn (e.g. native
    /// client hit `/shamir/v1/browser`).
    #[error("wrong path: expected {expected}, got {actual}")]
    WrongPath {
        /// Expected URI path.
        expected: &'static str,
        /// What the request actually had.
        actual: String,
    },
}

/// Accept a native-WSS upgrade on `/shamir/v1`.
///
/// `stream` is the TLS-wrapped TCP stream (post TLS 1.3 handshake).
/// Returns the upgraded `WebSocketStream`. Caller then extracts the TLS
/// exporter from the underlying TLS state for SCRAM channel binding.
pub async fn accept_native_ws<S>(stream: S) -> Result<WebSocketStream<S>, WsAcceptError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // NEW-1: cap tungstenite's per-connection buffering at the 16 MiB frame
    // ceiling (vs. the 64 MiB default) so an unauthenticated peer can't force
    // 64 MiB of buffering before the 4 KiB pre-auth logical check.
    let ws = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        |req: &Request, resp: Response| {
            // Spec §2: native endpoint expects path `/shamir/v1`.
            if req.uri().path() != "/shamir/v1" {
                let mut err = ErrorResponse::new(Some("wrong path for native endpoint".into()));
                *err.status_mut() = http::StatusCode::NOT_FOUND;
                return Err(err);
            }
            Ok(resp)
        },
        Some(server_ws_config()),
    )
    .await?;
    Ok(ws)
}

/// Accept a browser-WSS upgrade on `/shamir/v1/browser`.
///
/// Validates the `Origin` header against `policy` per spec §9. Rejects
/// upgrades without `Origin` (spec §9: native clients should use the
/// non-browser endpoint).
pub async fn accept_browser_ws<S>(
    stream: S,
    policy: &BrowserOriginPolicy,
) -> Result<WebSocketStream<S>, WsAcceptError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // We need to validate Origin *during* the handshake, BEFORE returning
    // 101 Switching Protocols. Tungstenite gives us a callback hook for
    // exactly this.
    let policy = policy.clone();
    // NEW-1: same buffering cap as the native path (see `server_ws_config`).
    let ws = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        move |req: &Request, resp: Response| {
            if req.uri().path() != "/shamir/v1/browser" {
                let mut err = ErrorResponse::new(Some("wrong path for browser endpoint".into()));
                *err.status_mut() = http::StatusCode::NOT_FOUND;
                return Err(err);
            }
            let origin = req
                .headers()
                .get(http::header::ORIGIN)
                .and_then(|v| v.to_str().ok());
            match validate_origin(&policy, origin) {
                Ok(()) => Ok(resp),
                Err(rej) => {
                    let mut err = ErrorResponse::new(Some(format!("origin rejected: {}", rej)));
                    *err.status_mut() = http::StatusCode::FORBIDDEN;
                    Err(err)
                }
            }
        },
        Some(server_ws_config()),
    )
    .await?;
    Ok(ws)
}
