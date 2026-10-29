use std::collections::BTreeMap;

use serde::Serialize;

use crate::admin::types::repl_ops::SeedRepositoryStatus;
use crate::admin::{CreatePublicationOp, CreateSubscriptionOp, ReplScope};
use crate::wire::repl::{ReplRepoInfo, ReplResponse};

/// Old-protocol fixtures: the shapes peers sent before the additive fields.
#[derive(Serialize)]
struct LegacyRepo {
    db: &'static str,
    repo: &'static str,
    current_version: u64,
    journal_floor: u64,
}

#[derive(Serialize)]
#[serde(tag = "repl_kind", rename_all = "snake_case")]
enum LegacyResponse {
    Hello {
        leader_epoch: u64,
        repos: Vec<LegacyRepo>,
    },
}

#[derive(Serialize)]
struct LegacySubscription {
    create_subscription: &'static str,
    upstream: &'static str,
    publication: &'static str,
    profile: &'static str,
}

#[test]
fn future_publication_fields_are_ignored_in_replication_response() {
    let response = serde_json::json!({
        "repl_kind": "hello",
        "leader_epoch": 1,
        "repos": [
            {
                "db": "app",
                "repo": "main",
                "current_version": 10,
                "journal_floor": 0,
                "publications": [
                    {
                        "create_publication": "pub",
                        "scopes": [
                            {
                                "db": "app",
                                "future_scope_field": true
                            }
                        ],
                        "future_publication_field": true
                    }
                ]
            }
        ]
    });
    let bytes = rmp_serde::to_vec_named(&response).unwrap();
    let decoded: ReplResponse = rmp_serde::from_slice(&bytes).unwrap();
    let ReplResponse::Hello { repos, .. } = decoded else {
        panic!("Hello");
    };
    assert_eq!(repos[0].publications[0].create_publication, "pub");
    assert_eq!(repos[0].publications[0].scopes[0].db, "app");
}

/// Serde round-trip test: the subject is the wire format itself.
#[test]
fn legacy_hello_and_subscription_decode_defaults_and_new_metadata_roundtrips() {
    let legacy = LegacyResponse::Hello {
        leader_epoch: 1,
        repos: vec![LegacyRepo {
            db: "app",
            repo: "main",
            current_version: 10,
            journal_floor: 0,
        }],
    };
    let old: ReplResponse =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&legacy).unwrap()).unwrap();
    let ReplResponse::Hello { repos, .. } = old else {
        panic!("Hello");
    };
    assert!(repos[0].schema_digest.is_none());
    assert!(repos[0].settled_version.is_none());
    assert!(repos[0].publications.is_empty());
    let legacy = LegacySubscription {
        create_subscription: "sub",
        upstream: "peer",
        publication: "pub",
        profile: "prof",
    };
    let subscription: CreateSubscriptionOp =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&legacy).unwrap()).unwrap();
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
    let empty = BTreeMap::<String, String>::new();
    let status: SeedRepositoryStatus =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&empty).unwrap()).unwrap();
    assert_eq!(status, SeedRepositoryStatus::default());
    let status = SeedRepositoryStatus {
        upstream_bookmark: Some(73),
        next_pull_version: Some(74),
        local_baseline_version: Some(1),
        local_consistent_version: Some(2),
        subscription_state: Some("paused".into()),
        ..status
    };
    let bytes = rmp_serde::to_vec_named(&status).unwrap();
    assert_eq!(
        rmp_serde::from_slice::<SeedRepositoryStatus>(&bytes).unwrap(),
        status
    );
    let bytes = rmp_serde::to_vec_named(&info).unwrap();
    assert_eq!(rmp_serde::from_slice::<ReplRepoInfo>(&bytes).unwrap(), info);
}
