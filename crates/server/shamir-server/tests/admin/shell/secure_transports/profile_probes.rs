//! Review items 5 and 6c: token-profile credential probes.
//!
//! 5 — a relative profile `ca_path` must give the SAME deterministic typed
//! refusal from two different child working directories; one directory
//! holds a VALID trust anchor under the old (CWD-following) behavior, so
//! the divergence proof is real: pre-fix the anchor directory succeeds and
//! the other fails.
//! 6c — `SHAMIR_SHELL_TOKEN` is read for the token transport only: a
//! positive token-auth run succeeds, the same variable under SCRAM auth is
//! refused with the typed usage record, and the token (and its suffix) is
//! never echoed anywhere.

use std::path::Path;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_query_builder::token_admin::create_api_token;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};

use crate::shell_tests::harness::{run_shell, ShellRun, DB_NAME};
use crate::shell_tests::secure_fixture::{
    boot_secure, provision_token, run_secret_process, scan_operator_home, secret_suffix,
    write_secure_profile,
};

use super::capture::{assert_clean, error, result};

const INVALID_PROFILE: &str = "the connection profile is invalid";

#[test]
fn shell_token_relative_ca_path_is_deterministic_refusal() {
    let fixture = boot_secure("shell_token_relative_ca_path_is_deterministic_refusal");
    let shell = &fixture.shell;
    // The bootstrap ADMIN exists by fixture construction; an unscoped
    // token keeps ping/connect free of any database-scoping dependence.
    let token = provision_token(
        shell,
        create_api_token("relative-ca")
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    write_secure_profile(
        shell,
        "https",
        fixture.http_addr,
        DB_NAME,
        Some(Path::new("ca.pem")),
    );

    // Directory A holds a VALID trust anchor named exactly like the
    // relative profile path: under the old CWD-following behavior this run
    // SUCCEEDS while directory B fails — the divergence this fix removes.
    let anchor_dir = TempDir::new().expect("anchor working directory");
    let pem = std::fs::read(&fixture.ca_path).expect("fixture trust anchor");
    std::fs::write(anchor_dir.path().join("ca.pem"), &pem).expect("write valid anchor");
    // Directory B is deliberately empty: no trust anchor under any name.
    let empty_dir = TempDir::new().expect("empty working directory");

    let refused_a = run_token_from(shell, &token, Some(anchor_dir.path()));
    error(&refused_a, "connect", 6, "invalid_profile", INVALID_PROFILE);
    let refused_b = run_token_from(shell, &token, Some(empty_dir.path()));
    error(&refused_b, "connect", 6, "invalid_profile", INVALID_PROFILE);
    // Deterministic: byte-identical records from both directories. The
    // assert message carries only lengths, never the compared bytes.
    assert!(
        refused_a.stdout == refused_b.stdout,
        "relative ca_path outcome depends on the working directory ({} vs {} stdout bytes)",
        refused_a.stdout.len(),
        refused_b.stdout.len(),
    );
}

fn run_token_from(
    shell: &crate::shell_tests::harness::ShellFixture,
    token: &Zeroizing<String>,
    current_dir: Option<&Path>,
) -> ShellRun {
    let run = run_secret_process(
        shell,
        Some(token.clone()),
        true,
        current_dir,
        &[],
        &["connect"],
    );
    let suffix = secret_suffix(token);
    assert!(!run.contains_canary(token) && !run.contains_canary(suffix));
    assert_clean(&run, token.as_str());
    scan_operator_home(shell, &[token.as_str(), suffix]);
    run
}

#[test]
fn shell_token_env_var_used_by_token_transport_only() {
    let fixture = boot_secure("shell_token_env_var_used_by_token_transport_only");
    let shell = &fixture.shell;
    // Unscoped token for the bootstrap ADMIN: connect/ping need nothing
    // beyond the accepted bearer credential.
    let token = provision_token(
        shell,
        create_api_token("env-probe")
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    write_secure_profile(
        shell,
        "https",
        fixture.http_addr,
        DB_NAME,
        Some(&fixture.ca_path),
    );
    // Set ONLY on the child: this process never carries the variable.
    let extra_env = [("SHAMIR_SHELL_TOKEN", token.as_str().to_owned())];

    // Positive: the token transport reads the auth-matched variable.
    let ok = run_secret_process(
        shell,
        None,
        true,
        None,
        &extra_env,
        &["--secret-env", "SHAMIR_SHELL_TOKEN", "--", "connect"],
    );
    let connected = result(&ok, "connect", None);
    assert!(connected["data"]["connected"].as_bool() == Some(true));
    assert_clean(&ok, token.as_str());
    assert!(
        !ok.contains_canary(secret_suffix(&token)),
        "token suffix echoed on a success stream"
    );
    scan_operator_home(shell, &[token.as_str(), secret_suffix(&token)]);

    // Negative: the SAME variable under SCRAM auth, pointed at a valid
    // SCRAM profile, is refused by the auth-matched name check AFTER the
    // command parses — one typed JSON usage record on stdout, exit 1, no
    // diagnostics on stderr, and NO echo of the token.
    write_secure_profile(shell, "wss", fixture.wss_addr, DB_NAME, None);
    let refused = run_secret_process(
        shell,
        None,
        false,
        None,
        &extra_env,
        &["--secret-env", "SHAMIR_SHELL_TOKEN", "--", "connect"],
    );
    error(&refused, "connect", 1, "usage", "invalid command usage");
    assert!(
        !refused.contains_canary(token.as_str()) && !refused.contains_canary(secret_suffix(&token)),
        "the token variable leaked through a refusal"
    );
    scan_operator_home(shell, &[token.as_str(), secret_suffix(&token)]);
    // The SCRAM positive control still works through the standard route;
    // the post-run home scan covers the last SCRAM WSS child of this test.
    let scram = run_shell(shell, &["connect"]);
    result(&scram, "connect", None);
    assert_clean(&scram, shell.canary.as_str());
    scan_operator_home(
        shell,
        &[shell.canary.as_str(), secret_suffix(&shell.canary)],
    );
}
