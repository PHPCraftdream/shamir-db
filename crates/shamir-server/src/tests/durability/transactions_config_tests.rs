use shamir_query_types::batch::VisibilityWait;

use crate::config::{Config, TransactionsConfig};

#[test]
fn transactions_full_example_and_custom_values_reach_config() {
    let example = include_str!("../../../../../deploy/server.example.ktav");
    let config: Config = ktav::from_str(example).unwrap();
    config.validate().unwrap();
    assert_eq!(config.transactions.visibility_ack_mode, "strict");
    assert_eq!(config.transactions.visibility_ack_timeout_ms, 5000);
    assert_eq!(config.transactions.max_visibility_wait_ms, 30000);
    assert!(config.transactions.allow_strict_visibility_wait);
    assert_eq!(config.transactions.read_visibility_timeout_ms, 5000);

    let custom = example
        .replace(
            "visibility_ack_timeout_ms: 5000",
            "visibility_ack_timeout_ms: 17",
        )
        .replace(
            "max_visibility_wait_ms: 30000",
            "max_visibility_wait_ms: 29",
        )
        .replace(
            "allow_strict_visibility_wait: true",
            "allow_strict_visibility_wait: false",
        )
        .replace(
            "read_visibility_timeout_ms: 5000",
            "read_visibility_timeout_ms: 23",
        );
    let config: Config = ktav::from_str(&custom).unwrap();
    config.validate().unwrap();
    assert_eq!(config.transactions.visibility_ack_timeout_ms, 17);
    assert_eq!(config.transactions.max_visibility_wait_ms, 29);
    assert!(!config.transactions.allow_strict_visibility_wait);
    assert_eq!(config.transactions.read_visibility_timeout_ms, 23);

    let bounded = custom.replace(
        "visibility_ack_mode: strict",
        "visibility_ack_mode: bounded",
    );
    let config: Config = ktav::from_str(&bounded).unwrap();
    assert_eq!(config.transactions.visibility_ack_mode, "bounded");
    config.validate().unwrap();
    assert_eq!(
        config.transactions.resolve_commit_wait(None).unwrap(),
        VisibilityWait::Bounded { timeout_ms: 17 },
    );
}

#[test]
fn transactions_legacy_defaults_zero_and_bounded_acceptance() {
    let example = include_str!("../../../../../deploy/server.example.ktav");
    let legacy = example
        .split("## Visibility acknowledgement")
        .next()
        .unwrap();
    let config: Config = ktav::from_str(legacy).unwrap();
    config.validate().unwrap();
    assert_eq!(config.transactions.visibility_ack_mode, "strict");
    assert_eq!(config.transactions.read_visibility_timeout_ms, 5000);
    assert_eq!(config.transactions.visibility_ack_timeout_ms, 5000);
    assert_eq!(config.transactions.max_visibility_wait_ms, 30000);
    assert!(config.transactions.allow_strict_visibility_wait);
    let parsed: TransactionsConfig =
        ktav::from_str("visibility_ack_mode: bounded\nvisibility_ack_timeout_ms: 0\n").unwrap();
    parsed.validate().unwrap();
    assert_eq!(
        parsed.resolve_commit_wait(None).unwrap(),
        VisibilityWait::Bounded { timeout_ms: 0 },
    );
    let mut zero = TransactionsConfig {
        visibility_ack_timeout_ms: 0,
        max_visibility_wait_ms: 0,
        read_visibility_timeout_ms: 0,
        ..TransactionsConfig::default()
    };
    zero.validate().unwrap();
    assert_eq!(
        zero.resolve_wait(VisibilityWait::Bounded { timeout_ms: 0 })
            .unwrap(),
        VisibilityWait::Bounded { timeout_ms: 0 },
    );
    assert_eq!(
        zero.resolve_wait(VisibilityWait::Bounded { timeout_ms: 1 })
            .unwrap_err()
            .code(),
        Some("visibility_wait_invalid"),
    );
    zero.allow_strict_visibility_wait = false;
    assert_eq!(
        zero.resolve_wait(VisibilityWait::Strict)
            .unwrap_err()
            .code(),
        Some("visibility_wait_invalid"),
    );
    zero.read_visibility_timeout_ms = u64::MAX;
    assert!(zero
        .validate()
        .unwrap_err()
        .to_string()
        .contains("read_visibility_timeout_ms"));
}

#[test]
fn transactions_validation_rejects_invalid_mode_cap_and_each_overflow() {
    let mut config = TransactionsConfig {
        visibility_ack_mode: "typo".into(),
        ..TransactionsConfig::default()
    };
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("strict or bounded"));
    config = TransactionsConfig::default();
    config.max_visibility_wait_ms = 4999;
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("exceeds"));
    for field in [
        "visibility_ack_timeout_ms",
        "max_visibility_wait_ms",
        "read_visibility_timeout_ms",
    ] {
        config = TransactionsConfig::default();
        match field {
            "visibility_ack_timeout_ms" => config.visibility_ack_timeout_ms = u64::MAX,
            "max_visibility_wait_ms" => config.max_visibility_wait_ms = u64::MAX,
            "read_visibility_timeout_ms" => config.read_visibility_timeout_ms = u64::MAX,
            _ => unreachable!("closed test input"),
        }
        assert!(config.validate().unwrap_err().to_string().contains(field));
    }
}
