//! Integration tests for `shamir_server::config`.
//!
//! Covers parse + validate round-trip: every test starts from a ktav
//! source string the way an operator would write it, deserializes via
//! `ktav::from_str`, then asserts on either the populated struct or
//! the [`ConfigError::Validation`] message.

use shamir_server::config::{Config, ConfigError, KdfConfig, ListenerKind, ProfileKind};

// ---------- helper: build a minimal valid base config string ----------

/// Minimal one-listener TLS-exporter ktav doc that should validate green.
/// Tests that exercise a single failure mode override one section and
/// keep the rest unchanged.
fn minimal_tcp_tls() -> &'static str {
    "\
data_dir: /var/lib/shamir-db

logging: {
    level: info
}

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

argon2_concurrent_max: 64

listeners: [
    {
        kind: tcp
        addr: 0.0.0.0:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
"
}

// =============== happy-path tests ===============

#[test]
fn parses_minimal_valid_config() {
    let cfg: Config = ktav::from_str(minimal_tcp_tls()).expect("parse ok");
    cfg.validate().expect("validate ok");

    assert_eq!(cfg.data_dir.to_str().unwrap(), "/var/lib/shamir-db");
    assert_eq!(cfg.logging.level, "info");
    assert_eq!(cfg.kdf_defaults.memory_kb, 131_072);
    assert_eq!(cfg.kdf_defaults.time, 4);
    assert_eq!(cfg.kdf_defaults.parallelism, 1);
    assert_eq!(cfg.kdf_defaults.argon2_version, 0x13);
    assert_eq!(cfg.argon2_concurrent_max, 64);
    assert_eq!(cfg.listeners.len(), 1);
    assert_eq!(cfg.listeners[0].kind, ListenerKind::Tcp);
    assert_eq!(cfg.listeners[0].profile, ProfileKind::TlsExporter);
    assert_eq!(cfg.listeners[0].addr, "0.0.0.0:7331");
    assert_eq!(
        cfg.tls.cert_path.to_str().unwrap(),
        "/var/lib/shamir-db/cert.pem"
    );
    assert_eq!(
        cfg.tls.key_path.to_str().unwrap(),
        "/var/lib/shamir-db/key.pem"
    );
}

#[test]
fn parses_full_example_with_all_listener_kinds() {
    let src = "\
data_dir: /var/lib/shamir-db

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

argon2_concurrent_max: 64

listeners: [
    {
        kind: tcp
        addr: 0.0.0.0:7331
        profile: tls_exporter
    }
    {
        kind: tcp
        addr: 127.0.0.1:7334
        profile: plain
    }
    {
        kind: ws
        addr: 0.0.0.0:7332
        profile: tls_exporter
        path: /shamir/v1
    }
    {
        kind: ws
        addr: 0.0.0.0:7333
        profile: tls_no_export
        path: /shamir/v1/browser
        browser_origin_allowlist: [
            https://app.example.com
            https://staging.example.com
        ]
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");

    assert_eq!(cfg.listeners.len(), 4);

    // Listener 0: native TCP+TLS exporter on public addr.
    assert_eq!(cfg.listeners[0].kind, ListenerKind::Tcp);
    assert_eq!(cfg.listeners[0].profile, ProfileKind::TlsExporter);
    assert!(cfg.listeners[0].path.is_none());

    // Listener 1: plain TCP on loopback.
    assert_eq!(cfg.listeners[1].kind, ListenerKind::Tcp);
    assert_eq!(cfg.listeners[1].profile, ProfileKind::Plain);

    // Listener 2: native WebSocket+TLS exporter.
    assert_eq!(cfg.listeners[2].kind, ListenerKind::Ws);
    assert_eq!(cfg.listeners[2].profile, ProfileKind::TlsExporter);
    assert_eq!(cfg.listeners[2].path.as_deref(), Some("/shamir/v1"));

    // Listener 3: browser WebSocket (tls_no_export).
    assert_eq!(cfg.listeners[3].kind, ListenerKind::Ws);
    assert_eq!(cfg.listeners[3].profile, ProfileKind::TlsNoExport);
    assert_eq!(cfg.listeners[3].path.as_deref(), Some("/shamir/v1/browser"));
    assert_eq!(cfg.listeners[3].browser_origin_allowlist.len(), 2);
}

#[test]
fn accepts_default_log_level_when_omitted() {
    // No `logging:` block at all; default must kick in.
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
        addr: 0.0.0.0:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    assert_eq!(cfg.logging.level, "info");
    // argon2_concurrent_max also defaulted.
    assert_eq!(cfg.argon2_concurrent_max, 64);
    cfg.validate().expect("validate ok");
}

#[test]
fn kdf_override_applies_per_listener() {
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
        kind: ws
        addr: 0.0.0.0:7333
        profile: tls_no_export
        path: /shamir/v1/browser
        kdf_override: {
            memory_kb: 65536
            time: 4
            parallelism: 1
            argon2_version: 19
        }
        browser_origin_allowlist: [
            https://app.example.com
        ]
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");

    let l = &cfg.listeners[0];
    let over = l.kdf_override.as_ref().expect("override present");
    assert_eq!(
        *over,
        KdfConfig {
            memory_kb: 65536,
            time: 4,
            parallelism: 1,
            argon2_version: 0x13,
        }
    );
    // And the defaults are still the higher value, untouched.
    assert_eq!(cfg.kdf_defaults.memory_kb, 131_072);
}

#[test]
fn parses_allow_public_metrics_true_when_set() {
    // M-tier audit M5 follow-up: `observability.allow_public_metrics`
    // must round-trip to `true` when the operator explicitly opts in —
    // this is the config-file-level knob that lets a non-loopback
    // observability bind through `server_launcher.rs`'s enforcement.
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
        addr: 0.0.0.0:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}

observability: {
    addr: 0.0.0.0:9090
    allow_public_metrics: true
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    cfg.validate().expect("validate ok");
    assert_eq!(cfg.observability.addr, "0.0.0.0:9090");
    assert!(
        cfg.observability.allow_public_metrics,
        "allow_public_metrics: true in the source must round-trip to true"
    );
}

#[test]
fn defaults_allow_public_metrics_to_false_when_omitted() {
    // Regression: whether the whole `observability` block is omitted, or
    // present but without `allow_public_metrics`, the field must default
    // to `false` — preserving today's safe-by-default behavior for every
    // existing config file that doesn't mention this new knob.
    let cfg: Config = ktav::from_str(minimal_tcp_tls()).expect("parse ok");
    cfg.validate().expect("validate ok");
    assert!(
        !cfg.observability.allow_public_metrics,
        "allow_public_metrics must default to false when the whole \
         observability block is omitted"
    );

    let src_with_block_but_no_field = "\
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
        addr: 0.0.0.0:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}

observability: {
    addr: 127.0.0.1:9090
}
";
    let cfg2: Config = ktav::from_str(src_with_block_but_no_field).expect("parse ok");
    cfg2.validate().expect("validate ok");
    assert!(
        !cfg2.observability.allow_public_metrics,
        "allow_public_metrics must default to false when the \
         observability block is present but doesn't mention the field"
    );
}

#[test]
fn loads_from_file() {
    use std::io::Write;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shamir-server.ktav");
    {
        let mut f = std::fs::File::create(&path).expect("create file");
        f.write_all(minimal_tcp_tls().as_bytes()).expect("write");
    }
    let cfg = Config::from_file(&path).expect("from_file ok");
    cfg.validate().expect("validate ok");
    assert_eq!(cfg.listeners.len(), 1);
    assert_eq!(cfg.listeners[0].profile, ProfileKind::TlsExporter);
}

// =============== validation-failure tests ===============

#[test]
fn rejects_plain_on_non_loopback() {
    // Same as minimal, but profile=plain on 0.0.0.0 — must be refused.
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
        addr: 0.0.0.0:7334
        profile: plain
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    let err = cfg.validate().expect_err("must reject plain on 0.0.0.0");
    let msg = format!("{err}");
    assert!(matches!(err, ConfigError::Validation(_)), "got {err:?}");
    assert!(
        msg.contains("loopback"),
        "expected error message to mention `loopback`, got: {msg}"
    );
}

#[test]
fn rejects_browser_endpoint_without_origin_allowlist() {
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
        kind: ws
        addr: 0.0.0.0:7333
        profile: tls_no_export
        path: /shamir/v1/browser
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    let err = cfg.validate().expect_err("must reject browser w/o origin");
    assert!(matches!(err, ConfigError::Validation(_)), "got {err:?}");
    let msg = format!("{err}");
    assert!(
        msg.contains("browser_origin_allowlist"),
        "expected message to mention `browser_origin_allowlist`, got: {msg}"
    );
}

#[test]
fn rejects_kdf_below_floor() {
    let src = "\
data_dir: /var/lib/shamir-db

kdf_defaults: {
    memory_kb: 1024
    time: 4
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: tcp
        addr: 0.0.0.0:7331
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    let err = cfg.validate().expect_err("must reject low memory_kb");
    assert!(matches!(err, ConfigError::Validation(_)), "got {err:?}");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("kdf"),
        "expected message to contain `kdf`, got: {msg}"
    );
}

#[test]
fn rejects_invalid_addr() {
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
        addr: not-an-addr
        profile: tls_exporter
    }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path: /var/lib/shamir-db/key.pem
}
";
    let cfg: Config = ktav::from_str(src).expect("parse ok");
    let err = cfg.validate().expect_err("must reject bad addr");
    assert!(matches!(err, ConfigError::Validation(_)), "got {err:?}");
    let msg = format!("{err}");
    assert!(
        msg.contains("not-an-addr") || msg.contains("socket address"),
        "expected message to mention bad addr, got: {msg}"
    );
}
