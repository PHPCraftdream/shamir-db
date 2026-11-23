//! R5 PR5 shell maintenance version tests: the boot-captured format-stamp
//! view (`version stamp`), the LOCAL database-identity validation that
//! precedes any connection, the identity-mismatch status outcome (a
//! mismatched identity is a typed status leaf, never an empty journal), and
//! the follower refusal of the status read.
//!
//! Drives the REAL production shell through the shared helpers of
//! [`super::harness`]. No test here asserts on timing: every fixture await
//! is a bounded hang detector, and the assertions cover payload shape,
//! typed codes and exit codes only.

use serde_json::Value;

use super::harness::{
    boot, boot_follower, overwrite_profile, profile_text, run_shell, ShellFixture,
};
use super::state_data_common::{data, refusal, STAMP_OUTCOMES};

/// The boot-created database every maintenance batch rides on (the same
/// value the harness seeding batches use): it exists on a fresh fixture, so
/// the profile's routing-database floor is available without a catalogue
/// seed.
const ROUTING_DB: &str = "default";

/// A well-formed Base58 `RecordId` that is not any fixture database's
/// identity: the all-zero 16-byte id renders as sixteen `1` characters.
const FOREIGN_DATABASE_ID: &str = "1111111111111111";

// ── Local shape helpers ──────────────────────────────────────────────

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

/// Point the shell's profile at the bootstrap admin over TLS-TCP with the
/// boot-created [`ROUTING_DB`] routing database and the fixture's pin.
fn use_routing_profile(fixture: &ShellFixture) {
    let text = profile_text(
        "tls-tcp",
        &fixture.tls_addr.to_string(),
        Some("localhost"),
        Some(ROUTING_DB),
    )
    .replacen("pin: <PIN>\n", &format!("pin: {}\n", fixture.pin_hex()), 1);
    overwrite_profile(fixture, &text);
}

// ── Tests ────────────────────────────────────────────────────────────

/// `version stamp` is a VIEW of the launcher-captured stamp: exit 0, the
/// stable command id and wire op, a string `server_version`, a decimal
/// string `format_epoch`, one of the documented `stamp_outcome` renderings
/// and a string `role`. The envelope's `node` object carries the OBSERVED
/// node identity (the P1 status view is the only one that has it), so
/// `node.role` is present — never an all-null object.
#[test]
fn version_stamp_reports_the_boot_captured_stamp() {
    let fixture = boot("version_stamp_reports_the_boot_captured_stamp");
    use_routing_profile(&fixture);

    fixture.guard.mark("maintenance: version stamp");
    let run = run_shell(&fixture, &["--", "version", "stamp"]);
    let payload = data(&run, "version");
    let envelope: Value = serde_json::from_slice(
        run.stdout
            .strip_suffix(b"\n")
            .expect("LF-terminated output"),
    )
    .expect("version stamp envelope is valid JSON");

    assert_eq!(
        envelope["command"].as_str(),
        Some("version stamp"),
        "version stamp lost its stable command id"
    );
    assert_eq!(
        envelope["op"].as_str(),
        Some("Status"),
        "version stamp must reuse the P1 Status op view"
    );
    assert!(
        payload["server_version"].as_str().is_some(),
        "version stamp `server_version` is not a string"
    );
    assert_decimal(&payload["format_epoch"], "version stamp `format_epoch`");
    let outcome = payload["stamp_outcome"].as_str().unwrap_or_default();
    assert!(
        STAMP_OUTCOMES.contains(&outcome),
        "version stamp reported an undocumented `stamp_outcome`"
    );
    assert!(
        payload["role"].as_str().is_some(),
        "version stamp `role` is not a string"
    );
    assert!(
        envelope["node"]["role"].as_str().is_some(),
        "version stamp envelope `node.role` is absent"
    );
}

/// A malformed `--database-id` is refused LOCALLY: exit 1 with the typed
/// `invalid_record_id` error envelope (and its `data` leaf null). No
/// profile is written for this fixture at all, so a shell that dialed first
/// would surface a profile or connection failure instead — the typed key
/// refusal can only come from the pre-connection validation.
#[test]
fn malformed_database_id_is_refused_locally() {
    let fixture = boot("malformed_database_id_is_refused_locally");

    fixture.guard.mark("maintenance: malformed database id");
    let run = run_shell(
        &fixture,
        &[
            "--",
            "schema-migration",
            "status",
            "--database-id",
            "not base58!",
        ],
    );
    refusal(&run, "schema-migration", "invalid_record_id", 1);
}

/// A well-formed database identity that is NOT the fixture database's
/// identity is a typed status leaf, not an empty journal: exit 0 with
/// `data.error` == `schema_migration_identity_mismatch`, the untouched
/// journal's `last_applied_ordinal` == `"0"` (decimal string) and a string
/// `checksum_validation`.
#[test]
fn identity_mismatch_is_not_an_empty_journal() {
    let fixture = boot("identity_mismatch_is_not_an_empty_journal");
    use_routing_profile(&fixture);

    fixture.guard.mark("maintenance: identity mismatch");
    let run = run_shell(
        &fixture,
        &[
            "--",
            "schema-migration",
            "status",
            "--database-id",
            FOREIGN_DATABASE_ID,
        ],
    );
    let payload = data(&run, "schema-migration");

    assert_eq!(
        payload["error"].as_str(),
        Some("schema_migration_identity_mismatch"),
        "a foreign identity must be a typed mismatch leaf"
    );
    assert_eq!(
        payload["last_applied_ordinal"].as_str(),
        Some("0"),
        "a mismatched identity must report the untouched ordinal as a decimal string"
    );
    assert!(
        payload["checksum_validation"].as_str().is_some(),
        "a mismatched identity must still report a string `checksum_validation`"
    );
}

/// A follower has no consistent journal+schema view, so the status READ is
/// refused on the server with the stable
/// `schema_migration_follower_read_only` code and §5 exit 5.
#[test]
fn follower_refuses_the_status_read() {
    let fixture = boot_follower("follower_refuses_the_status_read");
    use_routing_profile(&fixture);

    fixture.guard.mark("maintenance: follower status read");
    let run = run_shell(
        &fixture,
        &[
            "--",
            "schema-migration",
            "status",
            "--database-id",
            FOREIGN_DATABASE_ID,
        ],
    );
    refusal(
        &run,
        "schema-migration",
        "schema_migration_follower_read_only",
        5,
    );
}
