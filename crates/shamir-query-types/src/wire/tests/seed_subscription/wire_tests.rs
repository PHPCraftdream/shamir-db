use crate::admin::{CreatePublicationOp, CreateSubscriptionOp, ReplScope};
use crate::wire::repl::{ReplRepoInfo, ReplResponse};

/// Wire-format test: raw JSON is the independent old-protocol fixture, not a query.
#[test]
fn legacy_hello_and_subscription_decode_defaults_and_new_metadata_roundtrips() {
    let old: ReplResponse = serde_json::from_str(
        r#"{
        "repl_kind": "hello",
        "leader_epoch": 1,
        "repos": [{"db": "app", "repo": "main", "current_version": 10, "journal_floor": 0}]
    }"#,
    )
    .unwrap();
    let ReplResponse::Hello { repos, .. } = old else {
        panic!("Hello");
    };
    assert!(repos[0].schema_digest.is_none());
    assert!(repos[0].settled_version.is_none());
    assert!(repos[0].publications.is_empty());
    let subscription: CreateSubscriptionOp = serde_json::from_str(
        r#"{
        "create_subscription": "sub",
        "upstream": "peer",
        "publication": "pub",
        "profile": "prof"
    }"#,
    )
    .unwrap();
    assert!(!subscription.paused);
    let paused = CreateSubscriptionOp {
        paused: true,
        ..subscription
    };
    let bytes = rmp_serde::to_vec_named(&paused).unwrap();
    assert_eq!(
        rmp_serde::from_slice::<CreateSubscriptionOp>(&bytes).unwrap(),
        paused
    );
    let info = ReplRepoInfo {
        db: "app".into(),
        repo: "main".into(),
        current_version: 10,
        journal_floor: 0,
        repo_lineage_id: Some([1; 16]),
        history_epoch: Some(2),
        gap_first: Some(1),
        gap_through: Some(10),
        schema_digest: Some("b".repeat(64)),
        settled_version: Some(10),
        publications: vec![CreatePublicationOp {
            create_publication: "pub".into(),
            scopes: vec![ReplScope {
                db: "app".into(),
                repo: Some("main".into()),
                table: None,
            }],
        }],
    };
    let status: crate::admin::types::repl_ops::SeedRepositoryStatus =
        serde_json::from_str("{}").unwrap();
    assert_eq!(
        status,
        crate::admin::types::repl_ops::SeedRepositoryStatus::default()
    );
    let status = crate::admin::types::repl_ops::SeedRepositoryStatus {
        upstream_bookmark: Some(73),
        next_pull_version: Some(74),
        local_baseline_version: Some(1),
        local_consistent_version: Some(2),
        subscription_state: Some("paused".into()),
        ..status
    };
    let bytes = rmp_serde::to_vec_named(&status).unwrap();
    assert_eq!(
        rmp_serde::from_slice::<crate::admin::types::repl_ops::SeedRepositoryStatus>(&bytes)
            .unwrap(),
        status
    );
    let bytes = rmp_serde::to_vec_named(&info).unwrap();
    assert_eq!(rmp_serde::from_slice::<ReplRepoInfo>(&bytes).unwrap(), info);
}
