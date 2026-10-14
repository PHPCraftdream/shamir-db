//! Top-level config strictness: unknown top-level sections/keys must be
//! REJECTED at parse time (`#[serde(deny_unknown_fields)]` on `Config`),
//! so a typo like `durabilty = {}` fails at boot with the offending field
//! named in the error, instead of being silently ignored.

use shamir_server::config::Config;

/// Minimal valid ktav config (same shape as `tests/config.rs` fixtures).
fn minimal_valid() -> &'static str {
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
        addr: 127.0.0.1:7331
        profile: plain
    }
]

tls: {
    cert_path: cert.pem
    key_path: key.pem
}
"
}

#[test]
fn unknown_top_level_section_is_rejected() {
    let src = format!("{}\ndurabilty: {{\n}}\n", minimal_valid());
    let err = ktav::from_str::<Config>(&src).expect_err("typo section must fail at parse time");
    let msg = err.to_string();
    assert!(
        msg.contains("durabilty"),
        "parse error must name the unknown field, got {msg:?}"
    );
}

#[test]
fn unknown_top_level_key_is_rejected() {
    let src = format!("{}\nmax_connections: 10\n", minimal_valid());
    let err = ktav::from_str::<Config>(&src).expect_err("unknown key must fail at parse time");
    assert!(
        err.to_string().contains("max_connections"),
        "parse error must name the unknown field, got {:?}",
        err.to_string()
    );
}

#[test]
fn control_valid_config_parses_and_validates() {
    let cfg: Config = ktav::from_str(minimal_valid()).expect("valid config must parse");
    cfg.validate().expect("valid config must validate");
}
