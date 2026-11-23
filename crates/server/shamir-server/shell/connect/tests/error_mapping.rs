//! Unit tests of [`request_error`] in `connect/scram.rs` — the exact fixed
//! local SDK error values for the R5-2b misclassification fixes. Real token
//! channel/path behaviour is covered by the shell integration worker.

use shamir_client::ClientError;
use shamir_connect::Error as ConnectError;
use tokio_tungstenite::tungstenite::http::Response;
use tokio_tungstenite::tungstenite::Error as WsError;

use super::{request_error, server_code};
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

/// The ACL-mutation refusals keep their OWN typed error (and therefore their
/// own stable code and exit) instead of collapsing into `Server`; an
/// unlisted code still maps to `Server`, so the allowlist stays closed.
#[test]
fn acl_mutation_codes_are_typed() {
    assert_eq!(server_code("invalid_owner"), ShellError::InvalidOwner);
    assert_eq!(server_code("hmac_mismatch"), ShellError::HmacMismatch);
    assert_eq!(server_code("hmac_required"), ShellError::HmacRequired);
    assert_eq!(server_code("unknown_db"), ShellError::UnknownDb);
    assert_eq!(
        server_code("not_an_allowlisted_code"),
        ShellError::Server,
        "the allowlist must stay closed"
    );
}

/// A read-only follower's stable write refusal keeps its OWN typed error (the
/// design §6 follower diagnostic is surfaced, never retried or redirected), and
/// its exit code (design §5: a server-side refusal is exit 5). Mutant: falling
/// through to the generic `server_error` arm.
#[test]
fn read_only_replica_is_its_own_refusal() {
    for error in [
        ClientError::Db {
            code: "read_only_replica".to_string(),
            message: "server text is dropped".to_string(),
        },
        ClientError::Server {
            code: "read_only_replica".to_string(),
        },
    ] {
        let mapped = request_error(error);
        assert_eq!(mapped, ShellError::ReadOnlyReplica);
        assert_eq!(mapped.code(), "read_only_replica");
        assert_eq!(mapped.exit(), 5, "a server-side refusal is exit 5");
        assert!(
            !mapped.message().contains("dropped"),
            "the server text is never reproduced"
        );
    }
}

/// The VICTIM's own wire code `cancelled` (this session's in-flight batch was
/// cancelled by another operator) is its own typed refusal with exit 5, not a
/// generic server error — the fix must not confuse it with the control
/// command's `CancelOutcome::Cancelled` rendering.
#[test]
fn cancelled_batch_is_its_own_refusal() {
    let mapped = request_error(ClientError::Db {
        code: "cancelled".to_string(),
        message: "request cancelled".to_string(),
    });
    assert_eq!(mapped, ShellError::Cancelled);
    assert_eq!(mapped.code(), "cancelled");
    assert_eq!(mapped.exit(), 5);
    assert!(
        !mapped.message().contains("request cancelled"),
        "the server text is never reproduced"
    );
}

/// The legacy `not_found` fallback classifies a database, a store, a table and
/// a function alike, so its fixed text must name a resource — not a record or
/// a table. Mutant: the previous "the addressed record or table was not found".
#[test]
fn not_found_text_names_a_resource() {
    let mapped = request_error(ClientError::Db {
        code: "not_found".to_string(),
        message: "server text is dropped".to_string(),
    });
    assert_eq!(mapped, ShellError::NotFound);
    assert_eq!(mapped.exit(), 5);
    assert_eq!(mapped.message(), "the addressed resource was not found");
}
