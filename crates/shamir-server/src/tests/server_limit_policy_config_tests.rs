//! W4 config-boundary regressions: all policy fields are validated, including
//! fields absent from older configs. No filesystem or wall-clock oracle.

use crate::config::{Config, ConfigError, QueryLimitsConfig};
use crate::db_handler::QueryLimitsCap;
use shamir_query_types::batch::BatchLimits;

fn config() -> Config {
    ktav::from_str(
        "\
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
",
    )
    .expect("valid legacy config")
}

fn rejected(config: &Config, field: &str) {
    match config.validate() {
        Err(ConfigError::Validation(message)) => {
            assert!(message.contains(&format!("security.query_limits.{field}")));
            assert!(message.contains("must be in"));
        }
        other => panic!("expected field-specific validation error for {field}: {other:?}"),
    }
}

// Red before W4: validate accepted every above-ceiling value below.
#[test]
fn query_policy_rejects_every_field_above_engine_ceiling() {
    macro_rules! check {
        ($field:ident, $ceiling:ident) => {
            let mut config = config();
            config.security.query_limits.$field = BatchLimits::CEILING.$ceiling + 1;
            rejected(&config, stringify!($field));
        };
    }
    check!(max_result_size_bytes, max_result_size);
    check!(max_execution_time_secs, max_execution_time_secs);
    check!(max_queries_per_batch, max_queries);
    check!(max_nesting_depth, max_nesting_depth);
    check!(max_dependency_depth, max_dependency_depth);
    check!(max_iterations, max_iterations);
    check!(max_total_queries, max_total_queries);
    check!(max_total_work, max_total_work);
    check!(max_page_size, max_page_size);
    check!(max_offset, max_offset);
}

#[test]
fn query_policy_rejects_nonsensical_zero_values() {
    macro_rules! check {
        ($field:ident) => {
            let mut config = config();
            config.security.query_limits.$field = 0;
            rejected(&config, stringify!($field));
        };
    }
    check!(max_result_size_bytes);
    check!(max_execution_time_secs);
    check!(max_queries_per_batch);
    check!(max_iterations);
    check!(max_total_queries);
    check!(max_total_work);
    check!(max_page_size);
}

#[test]
fn query_policy_allows_meaningful_zero_and_ceiling_values() {
    let mut config = config();
    config.security.query_limits = QueryLimitsConfig {
        max_result_size_bytes: BatchLimits::CEILING.max_result_size,
        max_execution_time_secs: BatchLimits::CEILING.max_execution_time_secs,
        max_queries_per_batch: BatchLimits::CEILING.max_queries,
        max_nesting_depth: BatchLimits::CEILING.max_nesting_depth,
        max_dependency_depth: BatchLimits::CEILING.max_dependency_depth,
        max_iterations: BatchLimits::CEILING.max_iterations,
        max_total_queries: BatchLimits::CEILING.max_total_queries,
        max_total_work: BatchLimits::CEILING.max_total_work,
        max_page_size: BatchLimits::CEILING.max_page_size,
        max_offset: BatchLimits::CEILING.max_offset,
        max_inflight_response_bytes: None,
    };
    config.validate().expect("inclusive ceilings");
    assert_eq!(
        QueryLimitsCap::from(&config.security.query_limits)
            .policy()
            .caps,
        BatchLimits::CEILING,
    );
    config.security.query_limits.max_nesting_depth = 0;
    config.security.query_limits.max_dependency_depth = 0;
    config.security.query_limits.max_offset = 0;
    config
        .validate()
        .expect("zero disables nesting/dependencies/skipping");
}

#[test]
fn query_policy_legacy_partial_config_defaults_to_ceiling() {
    // Serde compatibility is the subject, not query construction.
    let limits: QueryLimitsConfig =
        ktav::from_str("max_queries_per_batch: 7").expect("old partial query_limits block");
    let caps = QueryLimitsCap::from(&limits).policy().caps;
    assert_eq!(caps.max_queries, 7);
    assert_eq!(caps.max_result_size, 64 * 1024 * 1024);
    assert_eq!(caps.max_execution_time_secs, 60);
    assert_eq!(
        caps.max_nesting_depth,
        BatchLimits::CEILING.max_nesting_depth
    );
    assert_eq!(
        caps.max_dependency_depth,
        BatchLimits::CEILING.max_dependency_depth,
    );
    assert_eq!(caps.max_iterations, BatchLimits::CEILING.max_iterations);
    assert_eq!(
        caps.max_total_queries,
        BatchLimits::CEILING.max_total_queries
    );
    assert_eq!(caps.max_total_work, BatchLimits::CEILING.max_total_work);
    assert_eq!(caps.max_page_size, BatchLimits::CEILING.max_page_size);
    assert_eq!(caps.max_offset, BatchLimits::CEILING.max_offset);
    config()
        .validate()
        .expect("omitted security block remains valid");
}
