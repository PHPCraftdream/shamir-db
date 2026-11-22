//! Unit tests of [`request_error`] in `connect/scram.rs` — the exact fixed
//! local SDK error values for the R5-2b misclassification fixes. Real token
//! channel/path behaviour is covered by the shell integration worker.

use shamir_client::ClientError;
use shamir_connect::Error as ConnectError;
use tokio_tungstenite::tungstenite::http::Response;
use tokio_tungstenite::tungstenite::Error as WsError;

use super::request_error;
use crate::shell::output::ShellError;

/// Native WSS upgrade rejected before SCRAM (404 from a listener on
/// another path): connection failure, not authentication — the exact wrap
/// produced by `upgrade_error` in `client/transport/dial_ws.rs`.
#[test]
fn wss_upgrade_failure_is_connection_not_authentication() {
    let response = Response::builder()
        .status(404)
        .body(Some(Vec::new()))
        .expect("static 404 response");
    let error = ClientError::Handshake {
        context: "websocket upgrade: HTTP error: 404 Not Found".to_string(),
        source: Some(Box::new(WsError::Http(Box::new(response)))),
    };
    assert_eq!(request_error(error), ShellError::Connection);
}

/// A real SCRAM authentication failure (typed `ConnectError::AuthFailed`
/// wrapped under `Handshake.source` by auth_ok.rs) stays Authentication.
#[test]
fn scram_auth_failure_stays_authentication() {
    let error = ClientError::Handshake {
        context: "authentication failed".to_string(),
        source: Some(Box::new(ConnectError::AuthFailed)),
    };
    assert_eq!(request_error(error), ShellError::Authentication);
}

/// REST server answered 408 after the request was dispatched: unknown
/// outcome (§6), mapped to the Timeout refusal — not generic exit 5.
#[test]
fn http_408_after_dispatch_is_timeout() {
    let error = ClientError::Http {
        status: 408,
        code: None,
        retry_after: None,
    };
    assert_eq!(request_error(error), ShellError::Timeout);
}

/// Typed `token_channel_denied` (403, REST-only token on the token WSS
/// upgrade) decoded by the SDK as `Db`: permission refusal (exit 4).
#[test]
fn token_channel_denied_is_denied() {
    let error = ClientError::Db {
        code: "token_channel_denied".to_string(),
        message: "server text is dropped".to_string(),
    };
    assert_eq!(request_error(error), ShellError::Denied);
}
