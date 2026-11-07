//! Server-side WebSocket accept paths.
//!
//! Two endpoints per spec TRANSPORT_WS §2:
//!
//! - **`/shamir/v1`** (native): TLS 1.3 + WebSocket upgrade. Caller
//!   extracts the exporter via [`crate::tls_exporter::extract_tls_exporter_from_stream`]
//!   before or after upgrade; launcher chooses before.
//!
//! - **`/shamir/v1/browser`** (browser): TLS 1.3 + WebSocket upgrade +
//!   mandatory `Origin` header validation. The exporter is NOT used;
//!   `binding_mode = 0x02` and `tls_exporter_or_zeros = [0u8; 32]`.

use crate::browser::{validate_origin, BrowserOriginPolicy, OriginRejected};
use crate::framing::MAX_WS_FRAME_SIZE;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::WebSocketStream;

/// Native WS endpoint path (spec TRANSPORT_WS §2).
pub const NATIVE_WS_PATH: &str = "/shamir/v1";
/// Browser WS endpoint path (spec TRANSPORT_WS §2).
pub const BROWSER_WS_PATH: &str = "/shamir/v1/browser";
/// Mandatory WebSocket subprotocol (spec TRANSPORT_WS §2, RV-TRANSPORT-WS-C0C43A18).
pub const WS_SUBPROTOCOL: &str = "shamir-v1";

/// 400 for a missing/incompatible subprotocol offer.
fn subprotocol_reject(msg: &'static str) -> ErrorResponse {
    let mut err = ErrorResponse::new(Some(msg.to_string()));
    *err.status_mut() = http::StatusCode::BAD_REQUEST;
    err
}

/// RV-TRANSPORT-WS-C0C43A18: require the client to offer [`WS_SUBPROTOCOL`]
/// and echo it back in the response. Absent header or no matching token → 400.
#[allow(clippy::result_large_err)] // Err type is tungstenite's Callback trait contract
fn negotiate_subprotocol(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    // Validate EVERY header value (non-text is a 400) before selecting.
    let mut offered = false;
    for val in req.headers().get_all(http::header::SEC_WEBSOCKET_PROTOCOL) {
        let text = val
            .to_str()
            .map_err(|_| subprotocol_reject("non-text Sec-WebSocket-Protocol header"))?;
        offered |= text.split(',').map(str::trim).any(|p| p == WS_SUBPROTOCOL);
    }
    if offered {
        let (mut parts, body) = resp.into_parts();
        parts.headers.insert(
            http::header::SEC_WEBSOCKET_PROTOCOL,
            http::HeaderValue::from_static(WS_SUBPROTOCOL),
        );
        return Ok(Response::from_parts(parts, body));
    }
    Err(subprotocol_reject(
        "client must offer the shamir-v1 WebSocket subprotocol",
    ))
}

/// Build the [`WebSocketConfig`] applied to every accepted server stream.
///
/// Tungstenite's accept-time message and frame ceilings bound wire buffering
/// to 16 MiB plus the four-byte prefix, including fragmented messages.
/// The framing adapter enforces the limit supplied by its caller only after
/// message assembly; callers select any tighter pre-authentication limit.
pub(crate) fn server_ws_config() -> WebSocketConfig {
    // `WebSocketConfig` is `#[non_exhaustive]`: build from `Default` via the
    // setters so any other field keeps its default.
    // RV-TRANSPORT-WS-C038F1F3: the tungstenite ceilings bound the WIRE
    // message (`len_be || payload`), so they must admit the full 16 MiB
    // logical payload range plus the 4-byte prefix.
    let wire_ceiling = MAX_WS_FRAME_SIZE + 4;
    WebSocketConfig::default()
        .max_message_size(Some(wire_ceiling))
        .max_frame_size(Some(wire_ceiling))
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
        expected: String,
        /// What the request actually had.
        actual: String,
    },
}

/// Accept a native-WSS upgrade on `path` (spec default: [`NATIVE_WS_PATH`]).
///
/// `stream` is the TLS-wrapped TCP stream (post TLS 1.3 handshake).
/// Returns the upgraded `WebSocketStream`. Caller then extracts the TLS
/// exporter from the underlying TLS state for SCRAM channel binding.
///
/// Origin (spec §9): the header is OPTIONAL on the native endpoint. If the
/// client sends one, it must match `policy`; if absent, the upgrade is
/// accepted (native clients do not send `Origin`).
pub async fn accept_native_ws<S>(
    stream: S,
    path: &str,
    policy: &BrowserOriginPolicy,
) -> Result<WebSocketStream<S>, WsAcceptError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // NEW-1: cap tungstenite's per-connection buffering at the 16 MiB frame
    // ceiling (vs. the 64 MiB default) so an unauthenticated peer can't force
    // 64 MiB of buffering before the 4 KiB pre-auth logical check.
    let path = path.to_string();
    // Not a hot path: once-per-handshake capture, the callback and the
    // single awaiting task are the only users.
    let actual_path: Arc<Mutex<Option<String>>> = Arc::default();
    let origin_rej: Arc<Mutex<Option<OriginRejected>>> = Arc::default();
    let cb_path = path.clone();
    let cb_actual_path = actual_path.clone();
    let cb_origin_rej = origin_rej.clone();
    let ws = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        #[allow(clippy::result_large_err)] // Err type is tungstenite's Callback trait contract
        move |req: &Request, resp: Response| {
            *cb_actual_path.lock().expect("path capture poisoned") =
                Some(req.uri().path().to_string());
            // Spec §2: native endpoint expects the configured path.
            if req.uri().path() != cb_path {
                let mut err = ErrorResponse::new(Some("wrong path for native endpoint".into()));
                *err.status_mut() = http::StatusCode::NOT_FOUND;
                return Err(err);
            }
            // Spec §9: Origin optional here; if present it must be allowed.
            if let Some(origin_val) = req.headers().get(http::header::ORIGIN) {
                let supplied = origin_val.to_str().ok();
                let rejected = match supplied {
                    Some(origin) => validate_origin(policy, Some(origin)).err(),
                    // supplied but non-text: never treat as absent
                    None => Some(OriginRejected::NotAllowed("<non-utf8>".into())),
                };
                if let Some(rej) = rejected {
                    // Preserve the typed rejection for the caller
                    // (RV-TRANSPORT-WS-6C58DE5C).
                    *cb_origin_rej.lock().expect("origin capture poisoned") = Some(rej);
                    let mut err = ErrorResponse::new(Some("origin rejected".into()));
                    *err.status_mut() = http::StatusCode::FORBIDDEN;
                    return Err(err);
                }
            }
            negotiate_subprotocol(req, resp)
        },
        Some(server_ws_config()),
    )
    .await
    .map_err(|e| match e {
        tokio_tungstenite::tungstenite::Error::Http(ref resp)
            if resp.status() == http::StatusCode::NOT_FOUND =>
        {
            WsAcceptError::WrongPath {
                expected: path.clone(),
                actual: actual_path
                    .lock()
                    .expect("path capture poisoned")
                    .take()
                    .unwrap_or_default(),
            }
        }
        tokio_tungstenite::tungstenite::Error::Http(ref resp)
            if resp.status() == http::StatusCode::FORBIDDEN =>
        {
            // RV-TRANSPORT-WS-6C58DE5C: surface the callback's typed
            // rejection instead of collapsing into `Handshake`.
            let rej = origin_rej
                .lock()
                .expect("origin capture poisoned")
                .take()
                .unwrap_or(OriginRejected::Missing);
            WsAcceptError::OriginRejected(rej)
        }
        other => other.into(),
    })?;
    Ok(ws)
}

/// Accept a browser-WSS upgrade on `path` (spec default: [`BROWSER_WS_PATH`]).
///
/// Validates the `Origin` header against `policy` per spec §9. Rejects
/// upgrades without `Origin` (spec §9: native clients should use the
/// non-browser endpoint).
pub async fn accept_browser_ws<S>(
    stream: S,
    path: &str,
    policy: &BrowserOriginPolicy,
) -> Result<WebSocketStream<S>, WsAcceptError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // We need to validate Origin *during* the handshake, BEFORE returning
    // 101 Switching Protocols. Tungstenite gives us a callback hook for
    // exactly this.
    let path = path.to_string();
    // NEW-1: same buffering cap as the native path (see `server_ws_config`).
    // Not a hot path: once-per-handshake capture, the callback and the
    // single awaiting task are the only users.
    let actual_path: Arc<Mutex<Option<String>>> = Arc::default();
    let origin_rej: Arc<Mutex<Option<OriginRejected>>> = Arc::default();
    let cb_path = path.clone();
    let cb_actual_path = actual_path.clone();
    let cb_origin_rej = origin_rej.clone();
    let ws = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        #[allow(clippy::result_large_err)] // Err type is tungstenite's Callback trait contract
        move |req: &Request, resp: Response| {
            *cb_actual_path.lock().expect("path capture poisoned") =
                Some(req.uri().path().to_string());
            if req.uri().path() != cb_path {
                let mut err = ErrorResponse::new(Some("wrong path for browser endpoint".into()));
                *err.status_mut() = http::StatusCode::NOT_FOUND;
                return Err(err);
            }
            let result = match req.headers().get(http::header::ORIGIN) {
                None => validate_origin(policy, None),
                Some(value) => match value.to_str() {
                    Ok(origin) => validate_origin(policy, Some(origin)),
                    Err(_) => Err(OriginRejected::NotAllowed("<invalid header>".into())),
                },
            };
            match result {
                Ok(()) => negotiate_subprotocol(req, resp),
                Err(rej) => {
                    // Preserve the typed rejection for the caller
                    // (RV-TRANSPORT-WS-6C58DE5C).
                    *cb_origin_rej.lock().expect("origin capture poisoned") = Some(rej.clone());
                    let (status, msg) = match rej {
                        OriginRejected::Missing => (
                            http::StatusCode::BAD_REQUEST,
                            "origin required: use the native endpoint".to_string(),
                        ),
                        OriginRejected::NotAllowed(_) => (
                            http::StatusCode::FORBIDDEN,
                            "origin not allowed".to_string(),
                        ),
                    };
                    let mut err = ErrorResponse::new(Some(msg));
                    *err.status_mut() = status;
                    Err(err)
                }
            }
        },
        Some(server_ws_config()),
    )
    .await
    .map_err(|e| match e {
        tokio_tungstenite::tungstenite::Error::Http(ref resp)
            if resp.status() == http::StatusCode::NOT_FOUND =>
        {
            WsAcceptError::WrongPath {
                expected: path.clone(),
                actual: actual_path
                    .lock()
                    .expect("path capture poisoned")
                    .take()
                    .unwrap_or_default(),
            }
        }
        tokio_tungstenite::tungstenite::Error::Http(resp)
            if resp.status() == http::StatusCode::BAD_REQUEST
                || resp.status() == http::StatusCode::FORBIDDEN =>
        {
            // RV-TRANSPORT-WS-6C58DE5C: surface the callback's typed
            // rejection instead of collapsing into `Handshake`.
            let rej = origin_rej
                .lock()
                .expect("origin capture poisoned")
                .take()
                .unwrap_or(OriginRejected::Missing);
            WsAcceptError::OriginRejected(rej)
        }
        other => other.into(),
    })?;
    Ok(ws)
}
