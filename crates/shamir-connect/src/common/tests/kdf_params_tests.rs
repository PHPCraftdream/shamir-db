//! Tests for [`KdfParams`] validation (spec §3.7, §5.1.1).

use crate::common::kdf_params::{
    validate_client_kdf_safe, KdfParams, HARD_MAX_MEMORY_KB, HARD_MAX_TIME,
};
use crate::common::types::limits;

#[test]
fn default_passes_client_limits() {
    KdfParams::DEFAULT.validate_client_limits().unwrap();
}

#[test]
fn default_passes_server_floor() {
    KdfParams::DEFAULT.validate_server_floor().unwrap();
}

#[test]
fn rejects_above_memory_limit() {
    let mut p = KdfParams::DEFAULT;
    p.memory_kb = limits::KDF_MAX_MEMORY_KB + 1;
    assert!(p.validate_client_limits().is_err());
}

#[test]
fn rejects_above_time_limit() {
    let mut p = KdfParams::DEFAULT;
    p.time = limits::KDF_MAX_TIME + 1;
    assert!(p.validate_client_limits().is_err());
}

#[test]
fn rejects_above_parallel_limit() {
    let mut p = KdfParams::DEFAULT;
    p.parallelism = limits::KDF_MAX_PARALLEL + 1;
    assert!(p.validate_client_limits().is_err());
}

#[test]
fn rejects_unsupported_argon2_version() {
    let mut p = KdfParams::DEFAULT;
    p.argon2_version = 0x10; // v1.0 — not supported in v1 spec
    assert!(p.validate_client_limits().is_err());
}

#[test]
fn rejects_below_server_floor_memory() {
    let mut p = KdfParams::DEFAULT;
    p.memory_kb = limits::KDF_MIN_MEMORY_KB - 1;
    assert!(p.validate_server_floor().is_err());
}

#[test]
fn rejects_below_server_floor_time() {
    let mut p = KdfParams::DEFAULT;
    p.time = limits::KDF_MIN_TIME - 1;
    assert!(p.validate_server_floor().is_err());
}

#[test]
fn defaults_match_spec_section_3_7() {
    // Pinned: changes to defaults require spec update + version bump consideration.
    assert_eq!(KdfParams::DEFAULT.memory_kb, 131_072);
    assert_eq!(KdfParams::DEFAULT.time, 4);
    assert_eq!(KdfParams::DEFAULT.parallelism, 1);
    assert_eq!(KdfParams::DEFAULT.argon2_version, 0x13);
}

#[test]
fn validate_client_kdf_safe_accepts_default() {
    validate_client_kdf_safe(&KdfParams::DEFAULT).expect("defaults must fit safety cap");
}

#[test]
fn validate_client_kdf_safe_rejects_one_gib_memory() {
    // Malicious / misconfigured server publishes 1 GiB memory — client
    // must reject before launching Argon2id (M-tier audit M1).
    let mut p = KdfParams::DEFAULT;
    p.memory_kb = 1024 * 1024; // 1 GiB
    let err = validate_client_kdf_safe(&p).expect_err("1 GiB must be rejected");
    assert!(err.contains("memory"), "error mentions memory: {err}");
}

#[test]
fn validate_client_kdf_safe_rejects_excessive_time() {
    let mut p = KdfParams::DEFAULT;
    p.time = HARD_MAX_TIME + 1;
    let err = validate_client_kdf_safe(&p).expect_err("time over cap must be rejected");
    assert!(err.contains("time"), "error mentions time: {err}");
}

#[test]
fn validate_client_kdf_safe_boundary_memory_passes() {
    let mut p = KdfParams::DEFAULT;
    p.memory_kb = HARD_MAX_MEMORY_KB;
    validate_client_kdf_safe(&p).expect("exact cap must be accepted");
}
