//! Tests for the operator-facing config schema — code-level defaults and
//! the shipped resource profiles in `deploy/`.
//!
//! Covers the RI-8 default tightening (result-size cap 1 GiB → 64 MiB,
//! max_active_connections 10000 → 1000) and the two new example ktav
//! profiles (`server.small.example.ktav`, `server.medium.example.ktav`).

use std::path::{Path, PathBuf};

use crate::config::{
    Config, ConnectionSecurity, CursorLimitsConfig, QueryLimitsConfig, SecurityConfig,
};

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

    // F-11: every shipped profile must set a finite inflight-response budget
    // (4× max_result_size_bytes); never `None` again.
    assert_eq!(
        cfg.security.query_limits.max_inflight_response_bytes,
        Some(128 * 1024 * 1024),
        "small profile max_inflight_response_bytes"
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

    // F-11: every shipped profile must set a finite inflight-response budget
    // (4× max_result_size_bytes); never `None` again.
    assert_eq!(
        cfg.security.query_limits.max_inflight_response_bytes,
        Some(256 * 1024 * 1024),
        "medium profile max_inflight_response_bytes"
    );
}

// =============== RI-15 / F-29: max_inflight_response_bytes validation ======

/// F-29 (#822): the code-level default is now a FINITE value
/// (`4 * default_max_result_size_bytes()` = 256 MiB), not `None`/unbounded.
/// `ByteBudget::acquire` never hard-errors on exhaustion (it waits, bounded,
/// until room frees up — see `byte_budget.rs`), so this default change turns
/// unbounded memory growth into bounded backpressure rather than a new
/// rejection path for deployments that never configured this knob.
#[test]
fn default_max_inflight_response_bytes_is_finite_256_mib() {
    assert_eq!(
        QueryLimitsConfig::default().max_inflight_response_bytes,
        Some(256 * 1024 * 1024),
        "code-level default max_inflight_response_bytes must be finite \
         (4x the 64 MiB max_result_size_bytes default) after F-29"
    );
}

/// A minimal `.ktav` that OMITS `max_inflight_response_bytes` entirely must
/// resolve to the finite default via `#[serde(default = "...")]` — the same
/// omitted-key path a pre-F-29 config file would take on upgrade.
#[test]
fn omitted_max_inflight_response_bytes_resolves_to_finite_default() {
    let src = "\
data_dir: /var/lib/shamir-db

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: tcp
        addr: 127.0.0.1:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");
    assert_eq!(
        cfg.security.query_limits.max_inflight_response_bytes,
        Some(256 * 1024 * 1024),
        "omitting the key entirely must resolve to the finite F-29 default"
    );
}

/// F-29's escape hatch: an operator who EXPLICITLY sets
/// `max_inflight_response_bytes: null` must still get a genuinely unbounded
/// `Option<usize>` (`None`) — serde's `Option<T>` deserialization honors an
/// explicit null independently of `#[serde(default = "...")]` (the default
/// fn only ever runs when the key is missing entirely), so "key absent" and
/// "key explicitly null" do NOT collapse to the same non-distinguishable
/// case here. This is the deliberately-kept opt-back-into-unbounded path;
/// `server_launcher.rs` logs a `tracing::warn!` when it observes this
/// resolved `None` at boot (not independently asserted here — see this
/// crate's existing convention: no log-capture test harness exists yet, so
/// only the resolved `Option<usize>` value is checked, per this task's
/// brief).
#[test]
fn explicit_null_max_inflight_response_bytes_stays_unbounded() {
    let src = "\
data_dir: /var/lib/shamir-db

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: tcp
        addr: 127.0.0.1:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}

security: {
    query_limits: {
        max_inflight_response_bytes: null
    }
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");
    assert_eq!(
        cfg.security.query_limits.max_inflight_response_bytes, None,
        "explicit `max_inflight_response_bytes: null` must round-trip to \
         None (unbounded) — the escape hatch F-29 deliberately keeps"
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

// =============== CR-A3: max_cursor_page_size validation ===============

/// Default `max_cursor_page_size` is 10,000 — the operator-facing cap on
/// `CreateCursor`/`FetchNext`'s `page_size` field.
#[test]
fn default_max_cursor_page_size_is_10_000() {
    assert_eq!(CursorLimitsConfig::default().max_cursor_page_size, 10_000);
}

/// `max_cursor_page_size == 0` must be rejected at startup — a zero cap
/// would make every `CreateCursor`/`FetchNext` request unusable (there is no
/// valid `page_size` left in the `1..=0` range).
#[test]
fn max_cursor_page_size_zero_is_rejected() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");
    cfg.security.cursors.max_cursor_page_size = 0;

    let err = cfg
        .validate()
        .expect_err("max_cursor_page_size == 0 must fail validation");
    let message = err.to_string();
    assert!(
        message.contains("max_cursor_page_size"),
        "error message must name the offending field: {message}"
    );
}

/// `max_cursor_page_size >= 1` must pass validation.
#[test]
fn max_cursor_page_size_nonzero_is_accepted() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");

    cfg.security.cursors.max_cursor_page_size = 1;
    cfg.validate()
        .expect("max_cursor_page_size == 1 must pass validation");

    cfg.security.cursors.max_cursor_page_size = 50_000;
    cfg.validate()
        .expect("max_cursor_page_size == 50_000 must pass validation");
}

// =============== CR-C1 (#776): idle_timeout_secs / max_cursors_per_session ===

/// `idle_timeout_secs == 0` must be rejected at startup — a zero idle
/// timeout would evict every cursor almost immediately (the reaper sweeps
/// every few seconds), disabling the feature outright rather than expressing
/// deliberate operator intent.
#[test]
fn cursor_idle_timeout_secs_zero_is_rejected() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");
    cfg.security.cursors.idle_timeout_secs = 0;

    let err = cfg
        .validate()
        .expect_err("idle_timeout_secs == 0 must fail validation");
    let message = err.to_string();
    assert!(
        message.contains("idle_timeout_secs"),
        "error message must name the offending field: {message}"
    );
}

/// `idle_timeout_secs >= 1` must pass validation.
#[test]
fn cursor_idle_timeout_secs_nonzero_is_accepted() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");

    cfg.security.cursors.idle_timeout_secs = 1;
    cfg.validate()
        .expect("idle_timeout_secs == 1 must pass validation");

    cfg.security.cursors.idle_timeout_secs = 3600;
    cfg.validate()
        .expect("idle_timeout_secs == 3600 must pass validation");
}

/// `max_cursors_per_session == 0` must be rejected at startup — a zero cap
/// would make every `CreateCursor` fail with `cursor_limit_exceeded`,
/// silently disabling the whole feature.
#[test]
fn max_cursors_per_session_zero_is_rejected() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");
    cfg.security.cursors.max_cursors_per_session = 0;

    let err = cfg
        .validate()
        .expect_err("max_cursors_per_session == 0 must fail validation");
    let message = err.to_string();
    assert!(
        message.contains("max_cursors_per_session"),
        "error message must name the offending field: {message}"
    );
}

/// `max_cursors_per_session >= 1` must pass validation.
#[test]
fn max_cursors_per_session_nonzero_is_accepted() {
    let mut cfg = Config::from_file(&deploy_path("server.small.example.ktav"))
        .expect("server.small.example.ktav must parse");

    cfg.security.cursors.max_cursors_per_session = 1;
    cfg.validate()
        .expect("max_cursors_per_session == 1 must pass validation");

    cfg.security.cursors.max_cursors_per_session = 1000;
    cfg.validate()
        .expect("max_cursors_per_session == 1000 must pass validation");
}

// =============== F-15: enable_experimental_migration_api default/opt-in ====

/// Default `enable_experimental_migration_api` is `false` — the experimental
/// online storage-migration API must stay disabled unless an operator
/// explicitly opts in. Mirrors `default_max_inflight_response_bytes_is_none`'s
/// "safe-by-default" pinning.
#[test]
fn default_enable_experimental_migration_api_is_false() {
    assert!(
        !SecurityConfig::default().enable_experimental_migration_api,
        "experimental migration API must default to disabled (false)"
    );
}

/// No shipped example profile must set the flag to `true` — it is unsafe as
/// an always-on default (see KNOWN_LIMITATIONS.md §2). Pinning every shipped
/// profile here catches a future accidental opt-in in `deploy/`.
#[test]
fn shipped_profiles_leave_experimental_migration_api_disabled() {
    for name in [
        "server.example.ktav",
        "server.small.example.ktav",
        "server.medium.example.ktav",
    ] {
        let cfg = Config::from_file(&deploy_path(name)).expect("{name} must parse");
        assert!(
            !cfg.security.enable_experimental_migration_api,
            "{name} must not set enable_experimental_migration_api (must stay false by default)"
        );
    }
}

/// Minimal valid ktav with a `security:` block that OMITS the field — the
/// serde `#[serde(default)]` on the field must resolve it to `false`, so a
/// config that predates this knob keeps today's safe-by-default behavior.
#[test]
fn enable_experimental_migration_api_defaults_false_when_omitted() {
    let src = "\
data_dir: /var/lib/shamir-db

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: tcp
        addr: 127.0.0.1:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}

security: {
    auth_init_rate_per_second: 10
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");
    assert!(
        !cfg.security.enable_experimental_migration_api,
        "omitted enable_experimental_migration_api must default to false"
    );
}

/// When the operator explicitly sets `enable_experimental_migration_api:
/// true` inside `security:`, it must round-trip to `true` — this is the
/// config-file knob the live server reads at boot to opt into the
/// experimental migration API.
#[test]
fn enable_experimental_migration_api_parses_true_when_set() {
    let src = "\
data_dir: /var/lib/shamir-db

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: tcp
        addr: 127.0.0.1:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}

security: {
    enable_experimental_migration_api: true
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");
    assert!(
        cfg.security.enable_experimental_migration_api,
        "explicit enable_experimental_migration_api: true must round-trip to true"
    );
}
