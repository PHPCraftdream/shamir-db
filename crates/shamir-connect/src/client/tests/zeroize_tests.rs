//! Regression tests: password buffers are zeroized on EVERY exit path,
//! including early-error returns (RV-CONNECT-226DEDF4).

use crate::client::bootstrap::build_request as build_bootstrap_request;
use crate::client::changepw::build_request as build_changepw_request;
use crate::common::kdf_params::KdfParams;
use crate::common::types::{BindingMode, TransportKind};
use crate::common::username::NormalizedUsername;

fn assert_wiped(buf: &[u8], what: &str) {
    assert!(
        buf.iter().all(|&b| b == 0),
        "{what} must be zeroized on the early-error return path"
    );
}

#[test]
fn rv_connect_226dedf4_bootstrap_password_wiped_on_policy_error() {
    // Invalid NEW password (too short) -> validate_password fails before
    // Argon2id; the buffer must still be wiped.
    let mut password = vec![0x41u8; 16];
    let err = build_bootstrap_request(
        [0x11u8; 32],
        NormalizedUsername::from_normalized_unchecked("alice".into()),
        &mut password,
        KdfParams::DEFAULT,
    );
    assert!(err.is_err());
    assert_wiped(&password, "bootstrap password");
}

#[test]
fn rv_connect_226dedf4_bootstrap_password_wiped_on_kdf_limit_error() {
    // Valid password but KDF params exceed the client safety cap ->
    // validate_client_limits fails after policy validation; still wiped.
    let mut password = b"correct horse battery staple".to_vec();
    let bad_params = KdfParams {
        memory_kb: u32::MAX,
        time: 4,
        parallelism: 1,
        argon2_version: 0x13,
    };
    let err = build_bootstrap_request(
        [0x11u8; 32],
        NormalizedUsername::from_normalized_unchecked("alice".into()),
        &mut password,
        bad_params,
    );
    assert!(err.is_err());
    assert_wiped(&password, "bootstrap password (kdf-limit path)");
}

#[test]
fn rv_connect_226dedf4_changepw_passwords_wiped_on_policy_error() {
    // NEW password violates policy -> early return; BOTH buffers must be
    // wiped on that exit path.
    let mut old_password = vec![0x43u8; 24];
    let mut new_password = vec![0x44u8; 8]; // too short
    let err = build_changepw_request(
        &NormalizedUsername::from_normalized_unchecked("alice".into()),
        &[0x22u8; 32],
        &[0x33u8; 32],
        &[0x44u8; 32],
        &[0x55u8; 16],
        KdfParams::DEFAULT,
        TransportKind::Tcp,
        BindingMode::None,
        &[0x66u8; 32],
        &mut old_password,
        &mut new_password,
        KdfParams::DEFAULT,
    );
    assert!(err.is_err());
    assert_wiped(&old_password, "old password");
    assert_wiped(&new_password, "new password");
}
