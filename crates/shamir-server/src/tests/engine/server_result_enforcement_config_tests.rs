//! W4b: serde compatibility and shipped operator policy contracts.

use std::path::PathBuf;

use crate::config::{Config, QueryLimitsConfig};
use crate::db_handler::QueryLimitsCap;

#[test]
fn server_result_enforcement_defaults_true_and_explicit_false_round_trips() {
    let legacy: QueryLimitsConfig =
        ktav::from_str("max_queries_per_batch: 7").expect("legacy partial query_limits must parse");
    assert!(legacy.enforce_result_size);
    assert!(QueryLimitsConfig::default().enforce_result_size);
    assert!(QueryLimitsCap::from(&legacy).policy().enforce_result_size);

    for enabled in [false, true] {
        let source = format!("enforce_result_size: {enabled}");
        let parsed: QueryLimitsConfig = ktav::from_str(&source).expect("explicit flag");
        assert_eq!(parsed.enforce_result_size, enabled);
        assert_eq!(
            QueryLimitsCap::from(&parsed).policy().enforce_result_size,
            enabled,
        );
        // The config is Deserialize-only: round-trip the operator-visible bool,
        // without adding a serialization API to the production config type.
        let encoded = format!("enforce_result_size: {}", parsed.enforce_result_size);
        let decoded: QueryLimitsConfig = ktav::from_str(&encoded).expect("flag round-trip");
        assert_eq!(decoded.enforce_result_size, enabled);
    }
    assert!(!QueryLimitsCap::UNLIMITED.policy().enforce_result_size);
}

#[test]
fn server_result_enforcement_shipped_profiles_parse_and_validate() {
    for name in [
        "server.example.ktav",
        "server.small.example.ktav",
        "server.medium.example.ktav",
        "server.large.example.ktav",
    ] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy")
            .join(name);
        let source = std::fs::read_to_string(&path).expect("shipped profile");
        assert!(source.contains("enforce_result_size: true"), "{name}");
        let config = Config::from_file(&path).expect("profile must parse");
        config.validate().expect("profile must validate");
        assert!(config.security.query_limits.enforce_result_size, "{name}");
        assert!(
            QueryLimitsCap::from(&config.security.query_limits)
                .policy()
                .enforce_result_size
        );

        // Both flag values retain the existing minimum and reserve consistency.
        for enabled in [false, true] {
            let mut config = config.clone();
            config.security.query_limits.enforce_result_size = enabled;
            config.security.query_limits.max_result_size_bytes = 1;
            config.validate().expect("one-byte cap is coherent");
            config.security.query_limits.max_result_size_bytes = 0;
            assert!(config.validate().is_err(), "zero cap must remain invalid",);
            config.security.query_limits.max_result_size_bytes = 2;
            config.security.query_limits.max_inflight_response_bytes = Some(1);
            assert!(config.validate().is_err(), "reserve must admit the cap");
        }
    }
}
