//! Free-text activity fields are projected through an allowlist, never
//! echoed as-is (design `admin-shell.md` §4.5, the privacy rule): `user`
//! and `db` must match the engine's own component-name rule, `transport`
//! must be one of the three labels the server emits, `op_kind` must be a
//! wire operation discriminant. Anything else is the fixed `<invalid>`
//! placeholder, so a peer cannot smuggle credential-like text, a terminal
//! escape or an over-long blob into the operator's terminal.
//!
//! Every assertion below names the value it forbids, so deleting the
//! allowlist (or widening it to `output::sanitize`) fails the test instead
//! of merely changing a string's shape.

use serde_json::Value;
use shamir_query_types::admin::activity::{ActivityRequestState, ActivityRow, RequestExecId};

use super::{project_request, project_session};

/// The fixed placeholder every out-of-shape field renders as.
const INVALID: &str = "<invalid>";

/// A credential-looking blob: a token shape the shell must never echo.
const TOKEN_SHAPE: &str = "shk1.7f3a9c.4b1d2e6f8a0c";

/// One session row with the four free-text fields under test.
fn session_row(user: &str, transport: &str) -> ActivityRow {
    ActivityRow::Session {
        session_handle: 7,
        user: user.to_owned(),
        transport: transport.to_owned(),
        created_at_ns: 1,
        idle_ns: 2,
        open_transactions: 3,
        open_cursors: 4,
    }
}

/// One request row with the free-text fields under test.
fn request_row(user: &str, db: &str, op_kind: &str) -> ActivityRow {
    ActivityRow::Request {
        request_exec_id: RequestExecId(9),
        session_handle: Some(7),
        user: user.to_owned(),
        db: db.to_owned(),
        op_kind: op_kind.to_owned(),
        state: ActivityRequestState::WaitingLock,
        elapsed_ns: 5,
    }
}

/// Well-shaped fields pass through unchanged: the allowlist is a filter, not
/// a rewrite (mutant: replacing every field with the placeholder).
#[test]
fn documented_values_pass_through() {
    let session = project_session(&session_row("admin", "tcp")).expect("session projects");
    assert_eq!(session["user"], "admin");
    assert_eq!(session["transport"], "tcp");
    for transport in ["ws", "ipc"] {
        let row = project_session(&session_row("u-1_2", transport)).expect("projects");
        assert_eq!(row["transport"], transport, "documented transport");
        assert_eq!(row["user"], "u-1_2", "engine name rule passes");
    }
    let request = project_request(&request_row("admin", "shelldb", "write")).expect("projects");
    assert_eq!(request["user"], "admin");
    assert_eq!(request["db"], "shelldb");
    assert_eq!(request["op_kind"], "write");
    // The server's own admin labels carry a colon: they must not become
    // `<invalid>` (mutant: an op_kind rule without the colon).
    let admin =
        project_request(&request_row("admin", "shelldb", "admin:list_activity")).expect("projects");
    assert_eq!(admin["op_kind"], "admin:list_activity");
}

/// A credential-shaped value, a control/escape sequence and an over-long
/// value in ANY free-text field render as the one fixed placeholder (mutant:
/// echoing `sanitize`d wire text — an escape-only sanitiser still lets the
/// credential shape and the long blob through).
#[test]
fn hostile_values_render_the_placeholder() {
    let escape = "\u{1b}[2Jshamir";
    let long = "a".repeat(300);
    for user in [TOKEN_SHAPE, escape, &long, "", "with space", "ünïcode"] {
        let session = project_session(&session_row(user, "tcp")).expect("projects");
        assert_eq!(session["user"], INVALID, "hostile session user");
        assert!(
            !session["user"]
                .as_str()
                .unwrap_or_default()
                .contains("shk1"),
            "a credential shape must never be echoed"
        );
        let request = project_request(&request_row(user, "shelldb", "write")).expect("projects");
        assert_eq!(request["user"], INVALID, "hostile request user");
    }
    for db in [TOKEN_SHAPE, escape, &long, "", "db/../etc"] {
        let request = project_request(&request_row("admin", db, "write")).expect("projects");
        assert_eq!(request["db"], INVALID, "hostile request db");
    }
    for transport in ["quic", "Tcp", escape, "", TOKEN_SHAPE] {
        let session = project_session(&session_row("admin", transport)).expect("projects");
        assert_eq!(session["transport"], INVALID, "undocumented transport");
    }
    for op_kind in ["write\n", "Write", escape, "", TOKEN_SHAPE, &"x".repeat(64)] {
        let request = project_request(&request_row("admin", "shelldb", op_kind)).expect("projects");
        assert_eq!(request["op_kind"], INVALID, "undocumented op_kind");
    }
}

/// The placeholder is the ONLY substitution: every other leaf of a hostile
/// row still projects normally (mutant: refusing the whole row, which would
/// hide a real request from the operator).
#[test]
fn hostile_text_does_not_drop_the_row() {
    let projected: Value = project_request(&request_row(TOKEN_SHAPE, TOKEN_SHAPE, TOKEN_SHAPE))
        .expect("a hostile row still projects");
    assert_eq!(projected["kind"], "request");
    assert_eq!(projected["request_exec_id"], "9");
    assert_eq!(projected["state"], "waiting_lock");
    assert_eq!(projected["user"], INVALID);
    assert_eq!(projected["db"], INVALID);
    assert_eq!(projected["op_kind"], INVALID);
}
