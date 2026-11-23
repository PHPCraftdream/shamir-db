//! Guards around the secret-output destination and around the operator message
//! of a created-but-unaudited token.
//!
//! `audit_recovery_gate_tests.rs` is at the 1000-line layout limit, so these two
//! small assertions live here.

use super::super::{create, secret_output};
use shamir_connect::server::audit_chain::AuditError;
#[cfg(unix)]
use std::path::Path;

/// The backslash half of the trailing-separator rule is Windows-only: on Unix a
/// backslash is an ordinary file-name byte, so `secret\` is a legal destination
/// and must not be typed-refused.
///
/// Mutant killed on Unix/macOS: removing the `cfg!(windows)` gate in
/// `secret_output::validate_out_file` (refusing a trailing backslash on every
/// platform) turns this into `Err(TrailingSeparator)`. The `/` form is pinned by
/// `out_file_with_trailing_separator_is_refused`, which runs on every platform
/// and therefore cannot catch that mutant.
#[cfg(unix)]
#[test]
fn unix_backslash_destination_is_not_refused_as_a_trailing_separator() {
    let out = Path::new("secret\\");
    assert_eq!(
        out.as_os_str().as_encoded_bytes(),
        b"secret\\".as_slice(),
        "fixture sanity: the backslash must survive in the raw bytes"
    );
    // A data directory that does not exist has no reserved namespace, so only
    // the trailing-separator check could refuse this call.
    let missing = Path::new("shamir-data-dir-that-does-not-exist");
    let resolved = secret_output::validate_out_file(out, missing)
        .expect("a trailing backslash is an ordinary file-name byte on Unix");
    assert_eq!(resolved, out, "the destination must pass through unchanged");
}

/// The persisted/checkpoint branch of a created-but-unaudited token must state
/// the same operator facts as the latched branch: the audit event was not
/// written or cannot be certified, the secret was not delivered, no output file
/// was left behind, and the token must be revoked by id.
///
/// Mutant killed: keeping the old class-neutral "the audit checkpoint failed;
/// revoke it with ..." text, which names neither the missing audit event nor the
/// suppressed delivery, fails the first two assertions.
#[test]
fn persistence_branch_reports_the_same_unaudited_facts() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    let out = tmp.path().join("unaudited.out");
    let output = secret_output::prepare(Some(&out), true).expect("prepare");
    let msg = create::fail_after_create_for_test(
        output,
        Some(&out),
        &anyhow::Error::new(AuditError::Persistence("disk full".into())),
        "TOKENID",
    )
    .to_string();
    assert!(
        msg.contains("not written") || msg.contains("cannot certify"),
        "{msg}"
    );
    assert!(msg.contains("NOT delivered"), "{msg}");
    assert!(msg.contains("output file"), "{msg}");
    assert!(msg.contains("TOKENID"), "{msg}");
    assert!(msg.contains("token revoke"), "{msg}");
    assert!(
        !out.exists(),
        "the created-but-unusable output file must be removed"
    );
}
