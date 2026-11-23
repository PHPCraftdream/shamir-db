//! R5 PR5 shell maintenance tests: the online-copy trio (`backup start`,
//! `backup status`, `backup cancel`) driven against the REAL in-process
//! server through the shared helpers of [`super::harness`].
//!
//! These pin the online-copy command surface against the REAL server: the
//! typed status projection, the confirmation refusal before dispatch, the
//! server-side destination validation of an unmodified relative path, the
//! follower refusal, and the token refusal.

use std::path::Path;

use serde_json::Value;
use shamir_query_builder::token_admin;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use tempfile::TempDir;

use super::harness::{
    assert_exit, boot, boot_follower, overwrite_profile, profile_text, run_shell, seed_catalogue,
    seed_shell_user, ShellFixture, ShellRun, DB_NAME, SHELL_USER,
};
use super::secure_fixture::{boot_secure, provision_token, run_with_secret, write_secure_profile};
use super::state_data_common::scram_profile;

/// The launcher-created routing database. The follower fixture cannot be
/// seeded (a read-only node refuses the catalogue writes), so its profile
/// addresses the database every boot creates.
const DEFAULT_DB: &str = "default";

// ── Local shape helpers ──────────────────────────────────────────────

/// The single JSON Lines v1 record of one batch run: exactly one
/// LF-terminated line on stdout, wire version 1.
fn record(run: &ShellRun) -> Value {
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated output");
    assert!(!line.contains(&b'\n'), "expected exactly one record");
    let value = serde_json::from_slice::<Value>(&run.stdout).expect("invalid JSON record");
    assert_eq!(value["version"].as_u64(), Some(1), "wrong wire version");
    value
}

/// One batch run with the fixture's bootstrap secret scanned out of both
/// streams (the suite-wide leak discipline).
fn run_checked(fixture: &ShellFixture, args: &[&str]) -> ShellRun {
    let run = run_shell(fixture, args);
    assert!(
        !run.contains_canary(fixture.canary.as_str()),
        "the shell secret leaked"
    );
    run
}

/// One leaf that must render as a NONEMPTY string.
fn nonempty_text(leaf: &Value) -> bool {
    leaf.as_str().is_some_and(|text| !text.is_empty())
}

/// The DATA of one successful record: exit 0, the exact command id and op
/// label, `kind: result`, no error code, a data object and no batch
/// diagnostics.
fn result(fixture: &ShellFixture, args: &[&str], command: &str, op: &str) -> Value {
    let run = run_checked(fixture, args);
    assert_exit(&run, 0);
    let value = record(&run);
    assert_eq!(value["command"].as_str(), Some(command), "wrong command id");
    assert_eq!(
        value["kind"].as_str(),
        Some("result"),
        "not a result record"
    );
    assert_eq!(value["op"].as_str(), Some(op), "wrong op label");
    assert!(value["code"].is_null(), "a result record carries no code");
    assert!(
        value["data"].is_object(),
        "a result record carries no data object"
    );
    assert!(run.stderr.is_empty(), "unexpected batch diagnostics");
    value["data"].clone()
}

/// The whole ERROR record of one refused run: the exact exit, command id,
/// op label (`None` when the refusal happens before dispatch), code, and an
/// empty `data`.
fn refused_run(run: &ShellRun, command: &str, op: Option<&str>, code: &str, exit: i32) -> Value {
    assert_exit(run, exit);
    let value = record(run);
    assert_eq!(value["command"].as_str(), Some(command), "wrong command id");
    assert_eq!(value["kind"].as_str(), Some("error"), "not an error record");
    match op {
        Some(op) => assert_eq!(value["op"].as_str(), Some(op), "wrong op label"),
        None => assert!(
            value["op"].is_null(),
            "a pre-dispatch refusal carries no op"
        ),
    }
    assert_eq!(value["code"].as_str(), Some(code), "wrong refusal code");
    assert!(value["data"].is_null(), "an error record carries no data");
    value
}

/// [`refused_run`] for one command invoked against `fixture`.
fn refused(
    fixture: &ShellFixture,
    args: &[&str],
    command: &str,
    op: Option<&str>,
    code: &str,
    exit: i32,
) -> Value {
    let run = run_checked(fixture, args);
    refused_run(&run, command, op, code, exit)
}

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

/// Point a follower's profile at the bootstrap admin over TLS-TCP on the
/// launcher-created [`DEFAULT_DB`].
fn use_default_profile(fixture: &ShellFixture) {
    let text = profile_text(
        "tls-tcp",
        &fixture.tls_addr.to_string(),
        Some("localhost"),
        Some(DEFAULT_DB),
    )
    .replace("pin: <PIN>\n", &format!("pin: {}\n", fixture.pin_hex()));
    overwrite_profile(fixture, &text);
}

/// One absolute destination root inside a test-held [`TempDir`].
fn destination(dir: &TempDir) -> String {
    let path = dir.path().to_str().expect("the destination path is UTF-8");
    path.to_owned()
}

// ── Tests ────────────────────────────────────────────────────────────

/// A fresh fixture reports no running online copy: the typed status
/// projection answers `running: false` on a node that never started one.
#[test]
fn backup_status_reports_no_running_copy() {
    let fixture = boot("backup_status_reports_no_running_copy");
    seed_catalogue(&fixture);
    use_admin_profile(&fixture);

    let status = result(
        &fixture,
        &["--", "backup", "status"],
        "backup status",
        "BackupStatus",
    );
    assert_eq!(
        status["running"].as_bool(),
        Some(false),
        "a node that never started a copy reported a running one"
    );
}

/// An unconfirmed `backup start` is refused BEFORE any request: exit 6 with
/// the fixed `confirmation_required` code and no op label, and the node
/// still reports no running copy afterwards.
#[test]
fn backup_start_without_yes_is_refused_before_sending() {
    let destination_dir = TempDir::new().expect("destination tempdir");
    let fixture = boot("backup_start_without_yes_is_refused_before_sending");
    seed_catalogue(&fixture);
    use_admin_profile(&fixture);
    let to = destination(&destination_dir);

    refused(
        &fixture,
        &["--", "backup", "start", "--to", &to],
        "backup start",
        None,
        "confirmation_required",
        6,
    );
    let status = result(
        &fixture,
        &["--", "backup", "status"],
        "backup status",
        "BackupStatus",
    );
    assert_eq!(
        status["running"].as_bool(),
        Some(false),
        "an unconfirmed start must not leave a running copy"
    );
}

/// `backup start --yes` starts one copy: a nonempty backup id, the echoed
/// destination and its fixed note; a following status reports the run's
/// decimal-string `started_at_unix_ns` (no timing assertion).
#[test]
fn backup_start_with_yes_starts_a_copy() {
    let destination_dir = TempDir::new().expect("destination tempdir");
    let fixture = boot("backup_start_with_yes_starts_a_copy");
    seed_catalogue(&fixture);
    use_admin_profile(&fixture);
    let to = destination(&destination_dir);

    let started = result(
        &fixture,
        &["--", "backup", "start", "--to", &to, "--yes"],
        "backup start",
        "StartBackup",
    );
    assert!(
        nonempty_text(&started["backup_id"]),
        "backup start returned no backup id"
    );
    assert!(
        started["destination"].is_string(),
        "backup start lost its destination"
    );
    assert!(
        nonempty_text(&started["destination_note"]),
        "backup start lost its destination note"
    );

    let status = result(
        &fixture,
        &["--", "backup", "status"],
        "backup status",
        "BackupStatus",
    );
    let started_at = status["started_at_unix_ns"]
        .as_str()
        .expect("started_at_unix_ns is not a decimal string");
    assert!(
        !started_at.is_empty() && started_at.bytes().all(|byte| byte.is_ascii_digit()),
        "started_at_unix_ns is not a decimal string"
    );
}

/// An unconfirmed `backup cancel` is refused locally: exit 6 with the fixed
/// `confirmation_required` code and no op label.
#[test]
fn backup_cancel_without_yes_is_refused() {
    let fixture = boot("backup_cancel_without_yes_is_refused");
    seed_catalogue(&fixture);
    use_admin_profile(&fixture);

    refused(
        &fixture,
        &["--", "backup", "cancel"],
        "backup cancel",
        None,
        "confirmation_required",
        6,
    );
}

/// `backup cancel --yes` answers the node's current run: the typed boolean
/// outcome and the fixed run-scoped note (`cancel` carries no id).
#[test]
fn backup_cancel_with_yes_answers_the_current_run() {
    let destination_dir = TempDir::new().expect("destination tempdir");
    let fixture = boot("backup_cancel_with_yes_answers_the_current_run");
    seed_catalogue(&fixture);
    use_admin_profile(&fixture);
    let to = destination(&destination_dir);

    result(
        &fixture,
        &["--", "backup", "start", "--to", &to, "--yes"],
        "backup start",
        "StartBackup",
    );
    let cancelled = result(
        &fixture,
        &["--", "backup", "cancel", "--yes"],
        "backup cancel",
        "CancelBackup",
    );
    assert!(
        cancelled["cancelled"].is_boolean(),
        "the cancel outcome is not a boolean"
    );
    assert!(
        nonempty_text(&cancelled["current_run_note"]),
        "cancel lost its current-run note"
    );
}

/// A RELATIVE destination is passed UNMODIFIED and refused by the server's
/// own policy: exit 5 with `backup_invalid_destination`, and the shell never
/// created or resolved the path.
#[test]
fn relative_destination_is_refused_by_the_server_unmodified() {
    let fixture = boot("relative_destination_is_refused_by_the_server_unmodified");
    seed_catalogue(&fixture);
    use_admin_profile(&fixture);

    refused(
        &fixture,
        &["--", "backup", "start", "--to", "relative-dest", "--yes"],
        "backup start",
        Some("StartBackup"),
        "backup_invalid_destination",
        5,
    );
    assert!(
        !Path::new("relative-dest").exists(),
        "the shell created or resolved the relative destination"
    );
}

/// A follower (read-only node) refuses the node-local start/cancel with the
/// stable `online_backup_follower_read_only` code; the read-only status view
/// still answers.
#[test]
fn follower_refuses_start_and_cancel() {
    let destination_dir = TempDir::new().expect("destination tempdir");
    let fixture = boot_follower("follower_refuses_start_and_cancel");
    use_default_profile(&fixture);
    let to = destination(&destination_dir);

    refused(
        &fixture,
        &["--", "backup", "start", "--to", &to, "--yes"],
        "backup start",
        Some("StartBackup"),
        "online_backup_follower_read_only",
        5,
    );
    refused(
        &fixture,
        &["--", "backup", "cancel", "--yes"],
        "backup cancel",
        Some("CancelBackup"),
        "online_backup_follower_read_only",
        5,
    );
    result(
        &fixture,
        &["--", "backup", "status"],
        "backup status",
        "BackupStatus",
    );
}

/// §3.3: the maintenance commands are SCRAM only. Every online-copy command
/// (and the WAL diagnostic) is refused in a token session with the fixed
/// `token_request_denied` code and exit 4 BEFORE any request: the empty
/// `data` and the absent op label are the pre-dispatch evidence (the
/// counting-relay proof lives in `state_data::token_subset`). This also pins
/// the gate ORDER: the token refusal precedes both the explained payload and
/// the unsupported refusal, and no request is sent.
#[test]
fn token_session_is_refused_with_no_request() {
    let destination_dir = TempDir::new().expect("destination tempdir");
    let secure = boot_secure("token_session_is_refused_with_no_request");
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
    let to = destination(&destination_dir);
    let start = ["--", "backup", "start", "--to", to.as_str(), "--yes"];

    for (args, command) in [
        (&["--", "backup", "status"][..], "backup status"),
        (&start[..], "backup start"),
        (&["--", "backup", "cancel", "--yes"][..], "backup cancel"),
        (&["--", "backup", "verify", "SNAPSHOT"][..], "backup verify"),
        (&["--", "backup", "restore"][..], "backup restore"),
        (&["--", "backup", "snapshot"][..], "backup snapshot"),
        (
            &["--", "backup", "schedule", "show"][..],
            "backup schedule show",
        ),
        (&["--", "diag", "wal"][..], "diag wal"),
        (&["--", "diag", "durability", "main"][..], "diag durability"),
        (&["--", "diag", "integrity"][..], "diag integrity"),
        (&["--", "version", "stamp"][..], "version stamp"),
        (&["--", "version", "migrate"][..], "version migrate"),
        (
            &[
                "--",
                "schema-migration",
                "status",
                "--database-id",
                "1111111111111111",
            ][..],
            "schema-migration status",
        ),
        (&["--", "export", "logical"][..], "export logical"),
        (&["--", "import", "logical"][..], "import logical"),
    ] {
        fixture.guard.mark(format!("token: {}", args.join(" ")));
        let run = run_with_secret(fixture, token.clone(), true, args);
        assert!(
            !run.contains_canary(token.as_str()),
            "the token leaked in a refusal"
        );
        refused_run(&run, command, None, "token_request_denied", 4);
    }
}
