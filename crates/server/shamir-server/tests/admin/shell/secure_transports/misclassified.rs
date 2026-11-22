//! Review items 3a/3b: misclassified failures against the real listeners.
//!
//! 3a — a REST-only provisioned token used on the token WSS upgrade gets a
//! typed `token_channel_denied` (403) from the server; the shell must
//! surface the PERMISSION refusal (exit 4, fixed local message, no
//! reflection of the server text), never the generic server-error mapping.
//! 3b — a native WSS upgrade rejected BEFORE SCRAM (the decoy listener
//! serves a path the SDK never dials) is a CONNECTION failure (exit 2),
//! never an authentication failure.

use shamir_query_builder::token_admin::create_api_token;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};

use crate::shell_tests::harness::{run_shell, DB_NAME};
use crate::shell_tests::secure_fixture::{
    boot_secure, provision_token, run_token_shell, scan_operator_home, secret_suffix,
    write_secure_profile,
};

use super::capture::{assert_clean, error, result, CONNECT_FAILURE, DENIED};

#[test]
fn shell_token_wss_rest_only_token_channel_denied() {
    let fixture = boot_secure("shell_token_wss_rest_only_token_channel_denied");
    let shell = &fixture.shell;
    // An unscoped REST-ONLY token for the bootstrap ADMIN: the positive
    // control needs nothing beyond an accepted bearer credential on REST.
    let token = provision_token(
        shell,
        create_api_token("rest-only-channel")
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Rest),
    );

    // Positive control: the same REST-only token authenticates on HTTPS
    // REST and round-trips a Ping — the later refusal is attributable to
    // the CHANNEL, not the credential or the session.
    write_secure_profile(
        shell,
        "https",
        fixture.http_addr,
        DB_NAME,
        Some(&fixture.ca_path),
    );
    let run = |args: &[&str]| run_token_shell(shell, &token, args);
    let connected = result(&run(&["connect"]), "connect", None);
    assert!(connected["data"]["connected"].as_bool() == Some(true));
    result(&run(&["ping"]), "ping", Some("Ping"));

    // The refused route: the SAME token on the token WSS upgrade.
    write_secure_profile(
        shell,
        "token-wss",
        fixture.http_addr,
        DB_NAME,
        Some(&fixture.ca_path),
    );
    let refused = run_token_shell(shell, &token, &["connect"]);
    error(&refused, "connect", 4, "permission_denied", DENIED);
    assert!(
        !refused.contains_canary("token_channel_denied"),
        "server refusal text reflected on a child stream"
    );
}

#[test]
fn shell_native_wss_wrong_path_is_connection_failure() {
    let fixture = boot_secure("shell_native_wss_wrong_path_is_connection_failure");
    let shell = &fixture.shell;
    // The decoy listener is a REAL native WSS endpoint whose served path is
    // one the SDK's fixed upgrade route never matches, so the upgrade is
    // rejected before any SCRAM byte is sent.
    write_secure_profile(shell, "wss", fixture.decoy_wss_addr, DB_NAME, None);
    let run = run_shell(shell, &["connect"]);
    error(&run, "connect", 2, "connection_failed", CONNECT_FAILURE);
    assert!(
        !run.contains_canary("authentication_failed"),
        "pre-SCRAM upgrade rejection misreported as authentication"
    );
    assert_clean(&run, shell.canary.as_str());
    scan_operator_home(
        shell,
        &[shell.canary.as_str(), secret_suffix(&shell.canary)],
    );
}
