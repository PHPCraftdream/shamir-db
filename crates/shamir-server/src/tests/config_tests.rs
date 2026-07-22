//! Tests for the operator-facing config schema — code-level defaults and
//! the shipped resource profiles in `deploy/`.
//!
//! Covers the RI-8 default tightening (result-size cap 1 GiB → 64 MiB,
//! max_active_connections 10000 → 1000) and the two new example ktav
//! profiles (`server.small.example.ktav`, `server.medium.example.ktav`).

use std::path::{Path, PathBuf};

use crate::config::{Config, ConnectionSecurity, QueryLimitsConfig};

/// Resolve `<workspace>/deploy/<name>` from this crate's `CARGO_MANIFEST_DIR`.
///
/// Tests run with their CWD at the crate root (`crates/shamir-server/`), but
/// `cargo nextest` does not *guarantee* that across hosts — `CARGO_MANIFEST_DIR`
/// is set by cargo at compile time and is invariant, so it is the safest way
/// to reach a workspace-level file from a unit test.
fn deploy_path(name: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/shamir-server  →  workspace root.
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("deploy")
        .join(name)
}

// =============== code-level defaults (RI-8) ===============

#[test]
fn default_max_result_size_is_64_mib() {
    assert_eq!(
        QueryLimitsConfig::default().max_result_size_bytes,
        64 * 1024 * 1024,
        "code-level default max_result_size_bytes must be 64 MiB after RI-8"
    );
}

#[test]
fn default_max_active_connections_is_1000() {
    let cs = ConnectionSecurity::default();
    assert_eq!(
        cs.max_active_connections, 1_000,
        "code-level default max_active_connections must be 1000 after RI-8"
    );
    // RI-8 deliberately leaves the per-IP cap at 100 — it is already 10% of
    // the new global cap (and was 1% of the old one). Pinning it here guards
    // against an accidental "fix the ratio" follow-up that bumps it without a
    // deliberate spec change.
    assert_eq!(
        cs.max_active_connections_per_ip, 100,
        "default max_active_connections_per_ip must stay 100 (unchanged by RI-8)"
    );
}

// =============== shipped resource profiles ===============

#[test]
fn small_profile_parses_and_validates() {
    let cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");
    cfg.validate()
        .expect("server.small.example.ktav must pass Config::validate");

    // Argon2 auth-RAM ceiling = argon2_concurrent_max × memory_kb (KiB).
    // Pinned to the RI-8 brief's exact number so a future drift in the
    // shipped file is caught here, not just by re-reading the brief.
    let ceiling = cfg.argon2_concurrent_max as u64 * cfg.kdf_defaults.memory_kb as u64;
    assert_eq!(
        ceiling,
        6_u64 * 65_536,
        "small profile Argon2 ceiling must be 6 × 65536 KiB (got {ceiling})"
    );

    // The two fields RI-8 calls out explicitly for this profile.
    assert_eq!(
        cfg.security.connection.max_active_connections, 500,
        "small profile max_active_connections"
    );
    assert_eq!(
        cfg.security.query_limits.max_result_size_bytes,
        32 * 1024 * 1024,
        "small profile max_result_size_bytes"
    );
}

/// The all-fields reference example was previously never loaded by any
/// test — this is what let its `argon2_version: 19   # 0x13` /
/// `max_result_size_bytes: ... # 1 GiB` inline comments go unnoticed as a
/// latent parse bug (ktav only supports whole-line comments). Both were
/// moved onto their own comment line during RI-8 cleanup; this test pins
/// the file to actually being loadable so the regression can't return
/// silently.
#[test]
fn reference_example_parses_and_validates() {
    let cfg = Config::from_file(&deploy_path("server.example.ktav"))
        .expect("server.example.ktav must parse");
    cfg.validate()
        .expect("server.example.ktav must pass Config::validate");
}

#[test]
fn medium_profile_parses_and_validates() {
    let cfg = Config::from_file(&deploy_path("server.medium.example.ktav"))
        .expect("server.medium.example.ktav must parse");
    cfg.validate()
        .expect("server.medium.example.ktav must pass Config::validate");

    // Argon2 auth-RAM ceiling = argon2_concurrent_max × memory_kb (KiB).
    let ceiling = cfg.argon2_concurrent_max as u64 * cfg.kdf_defaults.memory_kb as u64;
    assert_eq!(
        ceiling,
        12_u64 * 131_072,
        "medium profile Argon2 ceiling must be 12 × 131072 KiB (got {ceiling})"
    );

    assert_eq!(
        cfg.security.connection.max_active_connections, 2_000,
        "medium profile max_active_connections"
    );
    assert_eq!(
        cfg.security.query_limits.max_result_size_bytes,
        64 * 1024 * 1024,
        "medium profile max_result_size_bytes"
    );
}

// =============== RI-15: max_inflight_response_bytes validation ===============

/// Default `max_inflight_response_bytes` is `None` (unbounded) — RI-15
/// must not change behavior for operators who don't opt in.
#[test]
fn default_max_inflight_response_bytes_is_none() {
    assert_eq!(
        QueryLimitsConfig::default().max_inflight_response_bytes,
        None,
        "default must be unbounded so RI-15 preserves pre-existing behavior"
    );
}

/// `max_inflight_response_bytes` set below `max_result_size_bytes` must be
/// rejected at startup — otherwise no single max-size batch response could
/// ever be admitted by the global budget gate.
#[test]
fn inflight_budget_below_result_cap_is_rejected() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");
    let result_cap = cfg.security.query_limits.max_result_size_bytes;
    cfg.security.query_limits.max_inflight_response_bytes = Some(result_cap - 1);

    let err = cfg
        .validate()
        .expect_err("max_inflight_response_bytes < max_result_size_bytes must fail validation");
    let message = err.to_string();
    assert!(
        message.contains("max_inflight_response_bytes"),
        "error message must name the offending field: {message}"
    );
}

/// `max_inflight_response_bytes` set equal to or above
/// `max_result_size_bytes` must pass validation.
#[test]
fn inflight_budget_at_or_above_result_cap_is_accepted() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");
    let result_cap = cfg.security.query_limits.max_result_size_bytes;

    cfg.security.query_limits.max_inflight_response_bytes = Some(result_cap);
    cfg.validate()
        .expect("max_inflight_response_bytes == max_result_size_bytes must pass validation");

    cfg.security.query_limits.max_inflight_response_bytes = Some(result_cap * 4);
    cfg.validate()
        .expect("max_inflight_response_bytes > max_result_size_bytes must pass validation");
}
