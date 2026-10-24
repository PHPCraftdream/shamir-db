use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;

use crate::replication::in_process::{InProcessReplSource, PullRetryHook, PULL_RETRY_HOOK};
use crate::replication::source::ReplSource;

async fn quarantined_source() -> (ShamirDb, InProcessReplSource) {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await.expect("create_db");
    db.add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let source = InProcessReplSource::new(db.clone());
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "crp",
        shamir_query_builder::ddl::replication_profile("prof_retry").stream(
            shamir_query_builder::ddl::repl_scope("app")
                .repo("main")
                .build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "cs",
        shamir_query_builder::ddl::subscription(
            "sub_retry",
            "tcp://leader:9000",
            "pub",
            "prof_retry",
        ),
    );
    db.execute("app", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    (db, source)
}

async fn flip_quarantine(db: &ShamirDb) {
    db.mark_subscription_resync_required("sub_retry", 100, 42)
        .await
        .unwrap();
}

#[tokio::test]
async fn in_process_hello_refuses_quarantined_subscription_before_advertising_repo() {
    let (db, source) = quarantined_source().await;
    flip_quarantine(&db).await;
    assert!(matches!(
        source.hello("follower").await.unwrap(),
        ReplResponse::Error { code, .. } if code == "downstream_resync_required"
    ));
}

#[tokio::test]
async fn maximum_pull_limit_on_empty_journal_returns_bounded_payload() {
    let db = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
        owner,
    )
    .await
    .unwrap();
    let source = InProcessReplSource::new(db);
    let response = source.pull("app", "main", 0, u32::MAX, None).await.unwrap();
    match response {
        ReplResponse::Pull { events, gap_at, .. } => {
            assert_eq!(events, vec![0x90]);
            assert_eq!(gap_at, None);
        }
        response => panic!("unexpected Pull response: {response:?}"),
    }
}

#[tokio::test]
async fn pending_in_process_long_poll_refuses_after_quarantine_flip() {
    let (db, source) = quarantined_source().await;
    let hook = std::sync::Arc::new(PullRetryHook {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let task_hook = hook.clone();
    let pending = tokio::spawn(async move {
        PULL_RETRY_HOOK
            .scope(task_hook, async move {
                source.pull("app", "main", u64::MAX, 10, Some(30_000)).await
            })
            .await
    });
    hook.entered.notified().await;
    flip_quarantine(&db).await;
    hook.release.notify_one();
    assert!(matches!(
        pending.await.unwrap().unwrap(),
        ReplResponse::Error { code, .. } if code == "downstream_resync_required"
    ));
}
