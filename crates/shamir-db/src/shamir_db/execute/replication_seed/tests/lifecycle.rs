#![cfg(unix)]

use std::sync::Arc;

use async_trait::async_trait;
use shamir_engine::meta::repl::bookmark_v3::{save_bookmark_v3, BookmarkV3};
use shamir_engine::repo::{BoxRepoFactory, RepoConfig, SeedImageWriter};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{paused_subscription, repl_scope, replication_profile};
use shamir_query_types::admin::{ReplDirection, ReplMode, SubAction};
use shamir_query_types::wire::repl::ReplResponse;

use super::super::catalogue::rows;
use super::validation::{hello, receipt, MetadataCase};
use crate::{ResumeProbe, SeedRefusal, ShamirDb};

struct Probe(ReplResponse);
#[async_trait]
impl ResumeProbe for Probe {
    async fn hello(&self, _: &str) -> Result<ReplResponse, SeedRefusal> {
        Ok(self.0.clone())
    }
}

pub(super) async fn home(complete: bool) -> ShamirDb {
    let home = ShamirDb::init_memory().await.unwrap();
    home.create_db("app").await.expect("create_db");
    home.add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let repo = home.get_db("app").unwrap().get_repo("main").unwrap();
    let _writer = SeedImageWriter::begin(repo, receipt(), Vec::new())
        .await
        .unwrap();
    if complete {
        _writer.finish(&[]).await.unwrap();
    }
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    home.execute(
        "app",
        &batch
            .to_request_via_msgpack()
            .expect("batch request encoding"),
    )
    .await
    .unwrap();
    home
}

async fn create(home: &ShamirDb) {
    let mut batch = Batch::new();
    batch.id(2);
    batch.op("sub", paused_subscription("sub", "peer", "pub", "prof"));
    home.execute(
        "app",
        &batch
            .to_request_via_msgpack()
            .expect("batch request encoding"),
    )
    .await
    .unwrap();
}

/// Metadata refusals are observed through the real repository and catalogue;
/// refusal must leave the complete subscription row byte-for-byte unchanged.
#[tokio::test]
async fn resume_refusal_does_not_mutate_and_recreate_still_checks_repository_receipt() {
    let home = home(true).await;
    create(&home).await;
    let before = rows(&home, "subscriptions").await.unwrap().remove(0);
    let mut local_wrong = before.clone();
    if let crate::types::value::QueryValue::Map(fields) = &mut local_wrong {
        fields.insert(
            "profile".into(),
            crate::types::value::QueryValue::Str("missing-profile".into()),
        );
    }
    assert_eq!(
        home.validate_seed_resume(&local_wrong).await,
        Err(SeedRefusal::WrongScope)
    );
    assert_eq!(rows(&home, "subscriptions").await.unwrap()[0], before);
    let cases: &[MetadataCase] = &[
        (|r| r.repo_lineage_id = None, SeedRefusal::MissingIdentity),
        (
            |r| r.repo_lineage_id = Some([9; 16]),
            SeedRefusal::WrongIdentity,
        ),
        (
            |r| r.history_epoch = Some(99),
            SeedRefusal::WrongHistoryEpoch,
        ),
        (|r| r.schema_digest = None, SeedRefusal::WrongSchema),
        (|r| r.publications.clear(), SeedRefusal::WrongPublication),
        (
            |r| r.publications[0].scopes[0].repo = Some("other".into()),
            SeedRefusal::WrongScope,
        ),
        (
            |r| r.gap_through = Some(11),
            SeedRefusal::HorizonAboveBookmark,
        ),
        (
            |r| r.settled_version = Some(9),
            SeedRefusal::SourceRegression,
        ),
    ];
    for (mutate, reason) in cases {
        let mut response = hello();
        if let ReplResponse::Hello { repos, .. } = &mut response {
            mutate(&mut repos[0]);
        }
        home.set_replication_resume_probe(Arc::new(Probe(response)));
        assert_eq!(
            home.validate_seed_resume_action(&before, &SubAction::Resume)
                .await,
            Err(*reason)
        );
        assert_eq!(rows(&home, "subscriptions").await.unwrap()[0], before);
    }
    let mut wrong = hello();
    if let ReplResponse::Hello { repos, .. } = &mut wrong {
        repos[0].repo_lineage_id = Some([9; 16]);
    }
    home.set_replication_resume_probe(Arc::new(Probe(wrong)));
    assert_eq!(
        home.validate_seed_resume_action(&before, &SubAction::Resume)
            .await,
        Err(SeedRefusal::WrongIdentity)
    );
    assert_eq!(rows(&home, "subscriptions").await.unwrap()[0], before);
    let mut recreated = before.clone();
    if let crate::types::value::QueryValue::Map(fields) = &mut recreated {
        fields.shift_remove("seed_receipt");
    }
    assert_eq!(
        home.validate_seed_resume(&recreated).await,
        Err(SeedRefusal::WrongIdentity)
    );
    home.set_replication_resume_probe(Arc::new(Probe(hello())));
    assert_eq!(home.validate_seed_resume(&before).await, Ok(()));
    let status = home.seed_status_entries(vec![before]).await.unwrap();
    assert_eq!(status[0]["upstream_bookmark"].as_i64(), Some(10));
    assert_eq!(status[0]["next_pull_version"].as_i64(), Some(11));
    assert_eq!(status[0]["local_baseline_version"].as_i64(), Some(1));
    assert_eq!(status[0]["local_consistent_version"].as_i64(), Some(1));
    assert_eq!(status[0]["subscription_state"].as_str(), Some("paused"));
    let executor = crate::shamir_db::execute::admin_dispatch::ShamirAdminExecutor {
        shamir: home.clone(),
        db_name: "app".into(),
        actor: crate::access::Actor::System,
        repo_admin_observer: None,
    };
    executor
        .handle_drop_subscription(&shamir_query_types::admin::DropSubscriptionOp {
            drop_subscription: "sub".into(),
        })
        .await
        .unwrap();
    create(&home).await;
    let recreated_row = rows(&home, "subscriptions").await.unwrap().remove(0);
    assert_eq!(home.validate_seed_resume(&recreated_row).await, Ok(()));
    let mut wrong = hello();
    if let ReplResponse::Hello { repos, .. } = &mut wrong {
        repos[0].repo_lineage_id = Some([9; 16]);
    }
    home.set_replication_resume_probe(Arc::new(Probe(wrong)));
    let refused = executor
        .handle_alter_subscription(&shamir_query_types::admin::AlterSubscriptionOp {
            alter_subscription: "sub".into(),
            action: SubAction::Resume,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(refused, crate::query::batch::BatchError::QueryError { code: Some(code), .. } if code == "seed_wrong_identity")
    );
    assert_eq!(
        rows(&home, "subscriptions").await.unwrap()[0]["state"].as_str(),
        Some("paused")
    );
    home.set_replication_resume_probe(Arc::new(Probe(hello())));
    executor
        .handle_alter_subscription(&shamir_query_types::admin::AlterSubscriptionOp {
            alter_subscription: "sub".into(),
            action: SubAction::Resume,
        })
        .await
        .unwrap();
    let active = rows(&home, "subscriptions").await.unwrap().remove(0);
    assert_eq!(active["state"].as_str(), Some("active"));
    assert_eq!(
        active["seed_resume_validated"],
        crate::types::value::QueryValue::Bool(true)
    );
}

#[tokio::test]
async fn bookmark_overflow_and_competing_owner_refuse_without_mutation() {
    let home = home(true).await;
    create(&home).await;
    let before = rows(&home, "subscriptions").await.unwrap().remove(0);
    let mut competing = before.clone();
    if let crate::types::value::QueryValue::Map(fields) = &mut competing {
        fields.insert(
            "name".into(),
            crate::types::value::QueryValue::Str("other".into()),
        );
    }
    super::super::catalogue::persist(&home, competing)
        .await
        .unwrap();
    assert_eq!(
        home.validate_seed_resume(&before).await,
        Err(SeedRefusal::CompetingOwner)
    );
    assert!(rows(&home, "subscriptions")
        .await
        .unwrap()
        .iter()
        .any(|row| row == &before));
    let isolated = self::home(true).await;
    create(&isolated).await;
    let before = rows(&isolated, "subscriptions").await.unwrap().remove(0);
    let repo = isolated.get_db("app").unwrap().get_repo("main").unwrap();
    // PR6b-3: the bookmark lives only as a strict v3 record now; seed it
    // beyond any possible source version (u64::MAX) so the seed-resume
    // bookmark overflow guard refuses — the scenario's original intent.
    let info = repo.tx_info_store().await.unwrap();
    save_bookmark_v3(
        &info,
        &BookmarkV3 {
            fence: None,
            bookmark: u64::MAX,
            clean_through: None,
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        isolated.validate_seed_resume(&before).await,
        Err(SeedRefusal::BookmarkOverflow)
    );
    assert_eq!(rows(&isolated, "subscriptions").await.unwrap()[0], before);
}

#[tokio::test]
async fn incomplete_install_refuses_creation_without_subscription_mutation() {
    let home = home(false).await;
    let op = shamir_query_types::admin::CreateSubscriptionOp {
        create_subscription: "sub".into(),
        upstream: "peer".into(),
        publication: "pub".into(),
        profile: "prof".into(),
        paused: true,
    };
    let mut record = crate::types::value::QueryValue::Null;
    assert_eq!(
        home.bind_seed_creation(&op, &mut record).await,
        Err(SeedRefusal::IncompleteInstall)
    );
    assert!(rows(&home, "subscriptions").await.unwrap().is_empty());
    let mut fields = crate::types::common::new_map();
    fields.insert(
        "name".into(),
        crate::types::value::QueryValue::Str("sub".into()),
    );
    fields.insert(
        "profile".into(),
        crate::types::value::QueryValue::Str("prof".into()),
    );
    fields.insert(
        "upstream".into(),
        crate::types::value::QueryValue::Str("peer".into()),
    );
    fields.insert(
        "publication".into(),
        crate::types::value::QueryValue::Str("pub".into()),
    );
    fields.insert(
        "state".into(),
        crate::types::value::QueryValue::Str("paused".into()),
    );
    let record = crate::types::value::QueryValue::Map(fields);
    super::super::catalogue::persist(&home, record.clone())
        .await
        .unwrap();
    assert_eq!(
        home.validate_seed_resume(&record).await,
        Err(SeedRefusal::IncompleteInstall)
    );
    assert_eq!(rows(&home, "subscriptions").await.unwrap()[0], record);
}
