use crate::config::{Config, ConfigError, DurabilityConfig};
use crate::server::{BootError, ServerLauncher};
use shamir_query_types::{ReplicationServe, RetentionOverflow, WalCommitTier};

const EXAMPLE: &str = include_str!("../../../../../../deploy/server.example.ktav");

#[test]
fn defaults_and_omitted_block_inherit_server_policy() {
    let cfg: DurabilityConfig = ktav::from_str("").unwrap();
    assert_eq!(cfg.wal_commit, WalCommitTier::Buffered);
    assert_eq!(cfg.wal_retention_max_bytes, 1_073_741_824);
    assert_eq!(
        cfg.on_wal_retention_exceeded,
        RetentionOverflow::RefuseWrites
    );
    assert_eq!(cfg.replication_serve, ReplicationServe::Durable);
    let legacy = EXAMPLE.replace("\r\n", "\n").replace(
        "durability: {\n    wal_commit: buffered\n    wal_retention_max_bytes: 1073741824\n    on_wal_retention_exceeded: refuse_writes\n    replication_serve: durable\n}",
        "",
    );
    assert!(!legacy.contains("durability: {"));
    let parsed: Config = ktav::from_str(&legacy).unwrap();
    assert_eq!(parsed.durability.wal_commit, WalCommitTier::Buffered);
    assert_eq!(parsed.durability.wal_retention_max_bytes, 1_073_741_824);
}

#[test]
fn unknown_nested_key_and_bad_enums_fail_real_ktav_parsing() {
    for input in [
        "wal_commmit: synced\n",
        "wal_commit: fast\n",
        "on_wal_retention_exceeded: delete\n",
        "replication_serve: completed\n",
    ] {
        assert!(
            ktav::from_str::<DurabilityConfig>(input).is_err(),
            "accepted {input}"
        );
    }
    let typo = EXAMPLE.replace("wal_commit: buffered", "wal_commmit: synced");
    assert!(ktav::from_str::<Config>(&typo).is_err());
}

#[test]
fn custom_values_and_effective_segment_floor_use_engine_resolver() {
    let cfg: DurabilityConfig = ktav::from_str(
        "wal_commit: synced\nwal_retention_max_bytes: 33554432\non_wal_retention_exceeded: declare_gap\n",
    ).unwrap();
    let policy = cfg.resolve_for_segment(16 * 1024 * 1024).unwrap();
    assert_eq!(policy.wal_commit(), WalCommitTier::Synced);
    assert_eq!(
        policy.on_wal_retention_exceeded(),
        RetentionOverflow::DeclareGap
    );
    assert_eq!(policy.replication_serve(), ReplicationServe::Durable);
    // Effective env override input, without mutating process-global env:
    // a larger effective segment invalidates the same configured budget.
    let err = cfg
        .resolve_for_segment(32 * 1024 * 1024)
        .unwrap_err()
        .to_string();
    assert!(err.contains("wal_retention_max_bytes"));
    assert!(err.contains("33554432"));
    assert!(err.contains("67108864"));
    let zero: DurabilityConfig = ktav::from_str("wal_retention_max_bytes: 0\n").unwrap();
    assert!(zero.resolve_for_segment(1).is_err());
}

/// PR8 chunk 1 un-gated `completed_reseed`: the server `durability:` section
/// with `replication_serve: completed_reseed` now parses, validates, and
/// resolves to `ReplicationServe::CompletedReseed`.
#[test]
fn completed_reseed_parses_and_resolves_to_completed_reseed_policy() {
    let input = EXAMPLE.replace(
        "replication_serve: durable",
        "replication_serve: completed_reseed",
    );
    let cfg: Config = ktav::from_str(&input).unwrap();
    cfg.validate().unwrap();
    assert_eq!(
        cfg.durability.replication_serve,
        ReplicationServe::CompletedReseed
    );
    let policy = cfg.durability.resolve_policy().unwrap();
    assert_eq!(
        policy.replication_serve(),
        ReplicationServe::CompletedReseed
    );
}

#[tokio::test]
async fn invalid_durability_is_rejected_before_boot_creates_storage() {
    // completed_reseed is VALID since PR8 chunk 1; an invalid durability that
    // still parses is a zero retention budget (under the 2× segment floor).
    let input = EXAMPLE.replace(
        "wal_retention_max_bytes: 1073741824",
        "wal_retention_max_bytes: 0",
    );
    let mut cfg: Config = ktav::from_str(&input).unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    cfg.data_dir = dir.path().join("must_not_be_created");
    let path = cfg.data_dir.clone();
    match ServerLauncher::new(cfg).launch().await {
        Err(BootError::Config(ConfigError::Validation(reason))) => {
            assert!(reason.contains("wal_retention_max_bytes"), "{reason}");
        }
        Err(other) => panic!("wrong boot error: {other}"),
        Ok(_) => panic!("unsupported durability started the server"),
    }
    assert!(
        !path.exists(),
        "invalid config created storage before rejection"
    );
}

#[test]
fn all_four_deployment_examples_parse_and_validate() {
    for example in [
        EXAMPLE,
        include_str!("../../../../../../deploy/server.small.example.ktav"),
        include_str!("../../../../../../deploy/server.medium.example.ktav"),
        include_str!("../../../../../../deploy/server.large.example.ktav"),
    ] {
        let cfg: Config = ktav::from_str(example).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.durability.wal_commit, WalCommitTier::Buffered);
        assert_eq!(cfg.durability.wal_retention_max_bytes, 1_073_741_824);
        assert_eq!(cfg.durability.replication_serve, ReplicationServe::Durable);
        assert_eq!(
            cfg.durability.on_wal_retention_exceeded,
            RetentionOverflow::RefuseWrites
        );
    }
}
