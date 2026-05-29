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
fn server_ws_config() -> WebSocketConfig {
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::Message;

    /// NEW-1: the server accept config MUST cap incoming buffering at the
    /// 16 MiB frame ceiling, strictly below tungstenite's 64 MiB
    /// `max_message_size` default. Otherwise an unauthenticated WSS peer can
    /// force ~64 MiB of buffering per connection before the framing layer's
    /// 4 KiB pre-auth logical check runs.
    #[test]
    fn server_config_caps_message_and_frame_at_16_mib() {
        let cfg = server_ws_config();
        assert_eq!(
            cfg.max_message_size,
            Some(MAX_WS_FRAME_SIZE),
            "max_message_size must be pinned to the 16 MiB frame ceiling",
        );
        assert_eq!(
            cfg.max_frame_size,
            Some(MAX_WS_FRAME_SIZE),
            "max_frame_size must be pinned to the 16 MiB frame ceiling",
        );

        // Strictly below the tungstenite defaults (msg 64 MiB / frame 16 MiB).
        let defaults = WebSocketConfig::default();
        assert!(
            cfg.max_message_size < defaults.max_message_size,
            "server max_message_size ({:?}) must be below the 64 MiB default ({:?})",
            cfg.max_message_size,
            defaults.max_message_size,
        );
        assert!(
            cfg.max_message_size <= Some(16 * 1024 * 1024),
            "server max_message_size must be <= 16 MiB",
        );
    }

    /// NEW-1 (live wiring): a real WS handshake against `accept_native_ws`
    /// must succeed AND the negotiated stream must reject a single message
    /// that exceeds the 16 MiB cap — proving `server_ws_config` is actually
    /// applied to the live accept path, not just returned by the helper.
    ///
    /// The client is configured with no frame/message cap so the oversized
    /// payload reaches the wire as one frame; the *server's* cap must be the
    /// thing that rejects it.
    #[tokio::test]
    async fn live_accept_rejects_message_over_cap() {
        let (server_io, client_io) = duplex(64 * 1024 * 1024);

        let server = tokio::spawn(async move {
            let mut ws = accept_native_ws(server_io).await.expect("accept");
            // Reading the oversized message must error (Capacity), never
            // yield a 16 MiB+ buffered payload.
            match ws.next().await {
                Some(Err(e)) => format!("err: {e}"),
                Some(Ok(m)) => panic!("expected error, got message len {}", m.len()),
                None => "closed".to_string(),
            }
        });

        // Client side: uncapped so it emits the oversized frame verbatim.
        let client_cfg = WebSocketConfig {
            max_message_size: None,
            max_frame_size: None,
            ..Default::default()
        };
        let (mut client, _resp) = tokio_tungstenite::client_async_with_config(
            "ws://localhost/shamir/v1",
            client_io,
            Some(client_cfg),
        )
        .await
        .expect("client handshake");

        // One byte over the server's 16 MiB ceiling.
        let oversized = vec![0xa5u8; MAX_WS_FRAME_SIZE + 1];
        // The send itself may surface the peer's protocol close; either way
        // the server task must report an error rather than a buffered body.
        let _ = client.send(Message::Binary(oversized)).await;

        let outcome = server.await.expect("server task join");
        assert!(
            outcome.starts_with("err:") || outcome == "closed",
            "server must reject the oversized message (got {outcome})",
        );
    }

    /// NEW-1 (live wiring, positive): a normal small frame still round-trips
    /// through the capped accept path — the cap must not break legitimate
    /// traffic.
    #[tokio::test]
    async fn live_accept_passes_small_frame() {
        let (server_io, client_io) = duplex(64 * 1024);

        let server = tokio::spawn(async move {
            let mut ws = accept_native_ws(server_io).await.expect("accept");
            match ws.next().await {
                Some(Ok(Message::Binary(b))) => b,
                other => panic!("expected binary, got {other:?}"),
            }
        });

        let (mut client, _resp) =
            tokio_tungstenite::client_async("ws://localhost/shamir/v1", client_io)
                .await
                .expect("client handshake");
        client
            .send(Message::Binary(b"hello".to_vec()))
            .await
            .expect("send");

        let got = server.await.expect("join");
        assert_eq!(got, b"hello");
    }
}
