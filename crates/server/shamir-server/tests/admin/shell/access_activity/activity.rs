//! R5 PR4 shell activity tests: the two operator activity views, the
//! cooperative cancellation surface and the SCRAM-only token refusal for the
//! activity commands. Drives the REAL production shell through the shared
//! helpers of [`super::state_data_common`].
//!
//! No test here needs a genuinely parked request: a cancel that observes a
//! live target requires a second session running a long request, which the
//! fixtures in this file cannot produce deterministically (the direct-wire
//! cancellation acceptance lives in `tests/admin/activity/`). The cancel
//! tests below therefore cover the local confirmation, the unknown-id
//! outcome and the local id validation only.

use serde_json::Value;
use shamir_query_builder::token_admin;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};

use super::harness::{
    overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture, ADMIN, DB_NAME,
    SHELL_USER,
};
use super::secure_fixture::{
    boot_secure, provision_token, run_token_shell, run_with_secret, write_secure_profile,
};
use super::state_data_common::{data, refusal, scram_profile, Relay, TOKEN_SUBSET};

// ── Local shape helpers ──────────────────────────────────────────────

/// Top-level fields of a projected listing (`project_page` in
/// `shell/commands/activity.rs`).
const PAGE_FIELDS: &[&str] = &["kind", "scope", "rows", "rows_returned", "rows_truncated"];

/// Fields of one projected session row (`project_session`).
const SESSION_FIELDS: &[&str] = &[
    "kind",
    "session_handle",
    "user",
    "transport",
    "created_at_ns",
    "idle_ns",
    "open_transactions",
    "open_cursors",
];

/// Fields of one projected request row (`project_request`).
const REQUEST_FIELDS: &[&str] = &[
    "kind",
    "request_exec_id",
    "session_handle",
    "user",
    "db",
    "op_kind",
    "state",
    "elapsed_ns",
];

/// The documented request-state renderings (`request_state_name`).
const REQUEST_STATES: &[&str] = &[
    "running",
    "done",
    "committing",
    "cancelled",
    "waiting_lock",
    "waiting_visibility",
    "waiting_budget",
];

/// Exactly the `expected` top-level keys of `value` — no missing, no extra.
fn keys_exact(value: &Value, expected: &[&str], context: &str) {
    let fields = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} is not a JSON object"));
    let mut actual = fields.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    let mut wanted = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(actual, wanted, "{context} field set");
}

/// One leaf that must render as a DECIMAL STRING (the shell's integer
/// rendering contract: a JSON string of ASCII digits, never a number).
fn assert_decimal(value: &Value, context: &str) -> String {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{context} is not a decimal string"));
    assert!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "{context} is not a decimal string"
    );
    text.to_owned()
}

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

// ── Tests ────────────────────────────────────────────────────────────

/// `activity sessions` in the default (own) scope reports the caller's own
/// session: the requested registry, the own scope, and one row for the
/// admin principal with every projected field in its documented rendering.
#[test]
fn activity_sessions_lists_own_session() {
    let secure = boot_secure("activity_sessions_lists_own_session");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    fixture.guard.mark("activity: sessions");
    let payload = data(&run_shell(fixture, &["activity", "sessions"]), "sessions");
    keys_exact(&payload, PAGE_FIELDS, "activity sessions");
    assert_eq!(payload["kind"], "sessions", "wrong listing registry");
    assert_eq!(payload["scope"], "own", "the default scope must be own");
    assert!(
        payload["rows_truncated"].is_boolean(),
        "`rows_truncated` is not a boolean"
    );
    let rows = payload["rows"]
        .as_array()
        .expect("activity sessions `rows` is not an array");
    assert_eq!(
        assert_decimal(&payload["rows_returned"], "rows_returned"),
        rows.len().to_string(),
        "`rows_returned` must be the decimal row count"
    );

    let mut own_session = false;
    for row in rows {
        keys_exact(row, SESSION_FIELDS, "activity sessions row");
        assert_eq!(row["kind"], "session", "wrong row kind in a sessions list");
        assert_decimal(&row["session_handle"], "session row `session_handle`");
        assert_decimal(&row["created_at_ns"], "session row `created_at_ns`");
        assert_decimal(&row["idle_ns"], "session row `idle_ns`");
        assert_decimal(&row["open_transactions"], "session row `open_transactions`");
        assert_decimal(&row["open_cursors"], "session row `open_cursors`");
        let transport = row["transport"]
            .as_str()
            .expect("session row `transport` is not a string");
        assert!(!transport.is_empty(), "session row `transport` is empty");
        let user = row["user"]
            .as_str()
            .expect("session row `user` is not a string");
        if user == ADMIN {
            own_session = true;
        }
    }
    assert!(
        own_session,
        "the admin principal's own session must appear in the own scope"
    );
}

/// `activity requests --limit 1` reports the requests registry with the two
/// scalar paging markers; the row set may be empty, but every returned row
/// must carry the documented request shape and a documented state word.
#[test]
fn activity_requests_reports_paging_marker() {
    let secure = boot_secure("activity_requests_reports_paging_marker");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    fixture.guard.mark("activity: requests");
    let payload = data(
        &run_shell(fixture, &["activity", "requests", "--limit", "1"]),
        "requests",
    );
    keys_exact(&payload, PAGE_FIELDS, "activity requests");
    assert_eq!(payload["kind"], "requests", "wrong listing registry");
    assert_eq!(payload["scope"], "own", "the default scope must be own");
    assert!(
        payload["rows_truncated"].is_boolean(),
        "`rows_truncated` is not a boolean"
    );
    let rows = payload["rows"]
        .as_array()
        .expect("activity requests `rows` is not an array");
    assert_eq!(
        assert_decimal(&payload["rows_returned"], "rows_returned"),
        rows.len().to_string(),
        "`rows_returned` must be the decimal row count"
    );

    for row in rows {
        keys_exact(row, REQUEST_FIELDS, "activity requests row");
        assert_eq!(row["kind"], "request", "wrong row kind in a requests list");
        assert_decimal(&row["request_exec_id"], "request row `request_exec_id`");
        assert_decimal(&row["elapsed_ns"], "request row `elapsed_ns`");
        if !row["session_handle"].is_null() {
            assert_decimal(&row["session_handle"], "request row `session_handle`");
        }
        let state = row["state"]
            .as_str()
            .expect("request row `state` is not a string");
        assert!(
            REQUEST_STATES.contains(&state),
            "request row `state` is not a documented rendering"
        );
    }
}

/// `--all` widens the scope across users, so the SERVER decides it: the
/// restricted principal is answered with the typed `permission_denied`
/// (exit 4), never a different local refusal code.
#[test]
fn activity_all_denied_without_activity_view_manage() {
    let secure = boot_secure("activity_all_denied_without_activity_view_manage");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let user_password = seed_shell_user(fixture);
    let restricted = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string())
        .replace("username: admin\n", &format!("username: {SHELL_USER}\n"));
    overwrite_profile(fixture, &restricted);

    fixture.guard.mark("activity: restricted --all");
    let run = run_with_secret(
        fixture,
        user_password.clone(),
        false,
        &["activity", "sessions", "--all"],
    );
    assert!(
        !run.contains_canary(user_password.as_str()),
        "the restricted user's secret leaked"
    );
    refusal(&run, "sessions", "permission_denied", 4);
}

/// An unconfirmed `activity cancel` is refused BEFORE any dial: exit 6 with
/// the fixed `confirmation_required` code, one error envelope, and the
/// counting relay observes ZERO connections.
#[test]
fn activity_cancel_without_yes_sends_nothing() {
    let secure = boot_secure("activity_cancel_without_yes_sends_nothing");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    let relay = Relay::start(fixture, fixture.tls_addr);
    let text = scram_profile(fixture, "tls-tcp", &relay.addr.to_string());
    overwrite_profile(fixture, &text);

    fixture.guard.mark("activity: cancel without --yes");
    let run = run_shell(fixture, &["activity", "cancel", "1"]);
    let accepted = relay.finish(fixture);
    let envelope = refusal(&run, "cancel", "confirmation_required", 6);
    assert!(
        envelope["data"].is_null(),
        "a confirmation refusal must carry no data leaf"
    );
    assert_eq!(
        accepted, 0,
        "an unconfirmed cancel must not open a downstream connection"
    );
}

/// An unknown request id is not an error: the server reports the typed
/// `already_finished` outcome and the shell exits 0 with only that leaf.
#[test]
fn activity_cancel_unknown_id_is_already_finished() {
    let secure = boot_secure("activity_cancel_unknown_id_is_already_finished");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    fixture.guard.mark("activity: cancel unknown id");
    let payload = data(
        &run_shell(fixture, &["activity", "cancel", "999999", "--yes"]),
        "cancel",
    );
    keys_exact(&payload, &["outcome"], "activity cancel");
    assert_eq!(
        payload["outcome"], "already_finished",
        "an unknown request id must report the already_finished outcome"
    );
}

/// The request id is validated LOCALLY, before any dial. A non-numeric id
/// reaches `parse_exec_id` (whose fixed refusal is `ShellError::Usage`) and
/// becomes the JSON `usage` envelope, exit 1. A leading `-1` is rejected by
/// the argv grammar itself, which is the fixed pre-parse refusal: exit 1,
/// the same `usage` code on stderr and NO stdout record.
#[test]
fn activity_cancel_rejects_malformed_id() {
    let secure = boot_secure("activity_cancel_rejects_malformed_id");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    fixture.guard.mark("activity: cancel non-numeric id");
    let run = run_shell(fixture, &["activity", "cancel", "abc", "--yes"]);
    refusal(&run, "cancel", "usage", 1);

    fixture.guard.mark("activity: cancel negative id");
    let negative = run_shell(fixture, &["activity", "cancel", "-1", "--yes"]);
    assert_eq!(
        negative.exit_code, 1,
        "a pre-parse usage refusal must exit 1"
    );
    assert!(
        negative.stdout.is_empty(),
        "a pre-parse usage refusal must emit no JSON record"
    );
    let stderr = String::from_utf8_lossy(&negative.stderr);
    assert!(
        stderr.starts_with("error: usage"),
        "the pre-parse refusal must carry the fixed usage code on stderr"
    );
}

/// §3.3: the activity commands are SCRAM only — every one of them is
/// refused in a token session with the fixed subset message and exit 4,
/// before any request.
#[test]
fn token_session_refuses_activity_commands() {
    let secure = boot_secure("token_session_refuses_activity_commands");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    let token = provision_token(
        fixture,
        token_admin::create_api_token("https")
            .for_user(SHELL_USER)
            .databases([DB_NAME])
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    write_secure_profile(
        fixture,
        "https",
        secure.http_addr,
        DB_NAME,
        Some(&secure.ca_path),
    );
    for (args, command_part) in [
        (&["activity", "sessions"][..], "sessions"),
        (&["activity", "requests"][..], "requests"),
        (&["activity", "cancel", "1", "--yes"][..], "cancel"),
    ] {
        fixture.guard.mark(format!("token: {}", args.join(" ")));
        let run = run_token_shell(fixture, &token, args);
        let value = refusal(&run, command_part, "token_request_denied", 4);
        assert!(
            value["message"].as_str() == Some(TOKEN_SUBSET),
            "wrong fixed token-subset message for `{}`",
            args.join(" ")
        );
    }
}

/// §3.3 for the activity commands: the token profile points at an address
/// NOTHING listens on, so any dial would surface as `connection_failed`
/// (exit 2); every command still exits 4 with the fixed subset message.
/// Mutant: a command that re-entered the token allowlist would dial and fail
/// with exit 2 instead.
#[test]
fn token_session_refuses_activity_commands_without_dialing() {
    let secure = boot_secure("token_session_refuses_activity_commands_without_dialing");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    let token = provision_token(
        fixture,
        token_admin::create_api_token("https")
            .for_user(SHELL_USER)
            .databases([DB_NAME])
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    // Port 1 on loopback has no listener: a dial here is `connection_failed`
    // (exit 2), so exit 4 proves the token gate refused BEFORE connecting.
    write_secure_profile(
        fixture,
        "https",
        std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
        DB_NAME,
        Some(&secure.ca_path),
    );
    for (args, command_part) in [
        (&["activity", "sessions"][..], "sessions"),
        (&["activity", "requests"][..], "requests"),
        (&["activity", "cancel", "1", "--yes"][..], "cancel"),
    ] {
        fixture.guard.mark(format!("token: {}", args.join(" ")));
        let run = run_token_shell(fixture, &token, args);
        let value = refusal(&run, command_part, "token_request_denied", 4);
        assert!(
            value["message"].as_str() == Some(TOKEN_SUBSET),
            "wrong fixed token-subset message for `{}`",
            args.join(" ")
        );
    }
}
