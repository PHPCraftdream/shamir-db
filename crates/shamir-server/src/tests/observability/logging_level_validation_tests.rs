//! `logging.level` must be validated at config-load time.
//!
//! `logging::init` consumes the level via `LevelFilter::from_str`, which
//! silently falls back to INFO on an unparseable value — a typo like
//! `warnn` would silently change the operator's verbosity. `Config::validate`
//! now rejects such values with a typed `ConfigError::Validation` naming the
//! valid levels.

use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigError};

/// Resolve `<workspace>/deploy/<name>` (same approach as `config_tests`).
fn deploy_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("deploy")
        .join(name)
}

fn valid_config() -> Config {
    Config::from_file(&deploy_path("server.example.ktav")).expect("server.example.ktav must parse")
}

#[test]
fn bad_level_is_rejected_and_lists_valid_levels() {
    let mut cfg = valid_config();
    cfg.logging.level = "warnn".into();
    match cfg.validate().expect_err("typo level must fail validation") {
        ConfigError::Validation(msg) => {
            assert!(msg.contains("logging.level"), "got {msg:?}");
            for valid in ["trace", "debug", "info", "warn", "error"] {
                assert!(
                    msg.contains(valid),
                    "error must list valid levels, missing {valid:?} in {msg:?}"
                );
            }
        }
        other => panic!("expected ConfigError::Validation, got {other:?}"),
    }
}

#[test]
fn every_valid_level_passes_validation() {
    for level in [
        "trace", "debug", "info", "warn", "error", "off", "INFO", "Warn",
    ] {
        let mut cfg = valid_config();
        cfg.logging.level = level.into();
        cfg.validate()
            .unwrap_or_else(|e| panic!("level {level:?} must pass validation: {e}"));
    }
}
