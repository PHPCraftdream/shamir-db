//! S5c consumer contracts over raw fjall + runtime teardown, not simulated gaps.
//! A separate durable marker fixture retains consumer/supervisor gap refusal.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig, RepoInstance};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::apply_replicated::{ApplyOutcome, ApplyPolicy};
use shamir_db::engine::tx::ReplicatedEvent;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    list_subscriptions, repl_scope, replication_profile, replication_status, subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

// The catalogue is fresh on each boot; the DATA repo is the same raw fjall
// directory. This isolates certification from metadata/file-lock retry timing.
async fn open(path: Option<&Path>) -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await;
    let factory = match path {
        Some(path) => BoxRepoFactory::fjall_raw(path),
        None => BoxRepoFactory::in_memory(),
    };
    db.add_repo(
        "app",
        RepoConfig::new("main", factory).add_table(TableConfig::new("items")),
    )
    .await
    .unwrap();
    db
}

fn repo(db: &ShamirDb) -> RepoInstance {
    db.get_db("app").unwrap().get_repo("main").unwrap()
}

async fn write(db: &ShamirDb, id: &str) -> u64 {
    let mut batch = Batch::new();
    batch.transactional();
    batch.insert(
        "put",
        insert("items").rows([doc! {
            "id" => id,
            "v" => 7,
        }]),
    );
    let response = db.execute("app", &batch.build()).await.unwrap();
    assert!(!response.results.contains_key("__error"), "{response:?}");
    let repo = repo(db);
    let h = repo.current_commit_version().await.unwrap();
    repo.flush_buffers().await.unwrap();
    let journal = repo.read_changelog_bounded(1, 100).await.unwrap();
    assert!(journal.confirmed, "fixture journal must confirm its prefix");
    assert_eq!(journal.bound, h);
    h
}

async fn rows(db: &ShamirDb) -> Vec<QueryValue> {
    let mut batch = Batch::new();
    batch.query("rows", Query::from("items"));
    let response = db.execute("app", &batch.build()).await.unwrap();
    let result = &response.results["rows"];
    assert!(!result.skipped);
    assert!(result.corrupt_records.is_empty(), "{result:?}");
    let mut rows: Vec<_> = result
        .records
        .iter()
        .map(|record| record.as_value().into_owned())
        .collect();
    rows.sort_by(|left, right| {
        left.get("id")
            .and_then(QueryValue::as_str)
            .cmp(&right.get("id").and_then(QueryValue::as_str))
    });
    rows
}

async fn run(follower: &ShamirDb, leader: &ShamirDb) -> Result<(), ReplError> {
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(InProcessReplSource::new(leader.clone())),
        FollowerLoopConfig::new("follower", "app", "main")
            .with_poll_wait_ms(0)
            .with_max_iterations(1),
        CancellationToken::new(),
    )
    .await
}

async fn create_subscription(follower: &ShamirDb) {
    let mut batch = Batch::new();
    batch.op(
        "profile",
        replication_profile("profile").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        subscription("sub", "tcp://leader:9000", "pub", "profile"),
    );
    let response = follower.execute("app", &batch.build()).await.unwrap();
    assert!(!response.results.contains_key("__error"), "{response:?}");
}

async fn assert_admin_state(follower: &ShamirDb, expected_state: &str) {
    let mut batch = Batch::new();
    batch.op("list", list_subscriptions());
    batch.op("status", replication_status());
    let response = follower.execute("app", &batch.build()).await.unwrap();
    for alias in ["list", "status"] {
        let value = response.results[alias].records[0].as_value();
        let QueryValue::List(subscriptions) = value.get("subscriptions").unwrap() else {
            panic!("expected subscription list: {value:?}");
        };
        assert_eq!(subscriptions.len(), 1);
        assert_eq!(
            subscriptions[0].get("name").and_then(QueryValue::as_str),
            Some("sub"),
        );
        assert_eq!(
            subscriptions[0].get("state").and_then(QueryValue::as_str),
            Some(expected_state),
        );
    }
}

#[test]
fn cold_reopen_reconciles_events_for_consumer_and_supervisor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("leader");
    let rt = runtime();
    let h = rt.block_on(async {
        let leader = open(Some(&path)).await;
        write(&leader, "before-crash").await
    });
    assert_eq!(h, 1);
    drop(rt);

    let rt = runtime();
    rt.block_on(async {
        let leader = open(Some(&path)).await;
        let source = InProcessReplSource::new(leader.clone());
        let reply = source.pull("app", "main", 1, 100, None).await.unwrap();
        let ReplResponse::Pull { events, gap_at, .. } = reply else {
            panic!("expected real pull");
        };
        assert_eq!(gap_at, None);
        let events: Vec<ReplicatedEvent> = rmp_serde::from_slice(&events).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.commit_version, h);
        let journal = leader
            .read_changelog_from_journal("app", "main", 1, 100)
            .await
            .unwrap();
        assert_eq!(journal.gap_at, None);
        assert_eq!(journal.events.len(), 1);
        assert_eq!(journal.events[0].event.commit_version, h);
        let follower = open(None).await;
        run(&follower, &leader).await.unwrap();
        assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), h);
        assert_eq!(rows(&follower).await, rows(&leader).await);

        let supervised = open(None).await;
        create_subscription(&supervised).await;
        let leader_clone = leader.clone();
        let factory: ReplSourceFactory = Arc::new(move |_| {
            Arc::new(InProcessReplSource::new(leader_clone.clone())) as Arc<dyn ReplSource>
        });
        let supervisor =
            SubscriptionSupervisor::new(Arc::new(supervised.clone()), factory, "follower")
                .with_poll_wait_ms(0)
                .with_max_iterations(1);
        supervisor.reconcile().await;
        supervisor.wait_for_subscription("sub").await;
        // The test join removes the task's handle after its configured pull iteration.
        assert_eq!(supervisor.active_count(), 0);
        assert_admin_state(&supervised, "running").await;
        assert_eq!(repo(&supervised).replication_bookmark().await.unwrap(), h);
        assert_eq!(rows(&supervised).await, rows(&leader).await);
        supervisor.stop_all().await;

        let later = write(&leader, "after-crash").await;
        assert!(later > h);
        let fresh = open(None).await;
        run(&fresh, &leader).await.unwrap();
        assert_eq!(repo(&fresh).replication_bookmark().await.unwrap(), later);
        assert_eq!(rows(&fresh).await, rows(&leader).await);
    });
    drop(rt);
}

#[test]
fn explicit_persisted_gap_stops_consumer_and_supervisor_but_not_beyond_h() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("leader");
    let rt = runtime();
    let h = rt.block_on(async {
        let leader = open(Some(&path)).await;
        write(&leader, "before-gap").await
    });
    drop(rt);

    let rt = runtime();
    rt.block_on(async {
        let leader = open(Some(&path)).await;
        let feed = repo(&leader).changefeed().await.unwrap();
        feed.feed.record_recovery_gap(1, h).await.unwrap();
        feed.feed.shutdown_and_wait().await.unwrap();
    });
    drop(rt);

    let rt = runtime();
    rt.block_on(async {
        let leader = open(Some(&path)).await;
        let source = InProcessReplSource::new(leader.clone());
        let reply = source.pull("app", "main", 1, 100, None).await.unwrap();
        let ReplResponse::Pull { events, gap_at, .. } = reply else {
            panic!("expected real pull");
        };
        assert_eq!(gap_at, Some(1));
        let events: Vec<ReplicatedEvent> = rmp_serde::from_slice(&events).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.commit_version, h);
        let follower = open(None).await;
        let error = run(&follower, &leader).await.unwrap_err();
        assert!(matches!(
            error,
            ReplError::JournalGap {
                gap_at: 1,
                from_version: 1
            }
        ));
        assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
        assert_eq!(repo(&follower).current_commit_version().await.unwrap(), 0);
        assert!(rows(&follower).await.is_empty());

        create_subscription(&follower).await;
        let leader_clone = leader.clone();
        let factory: ReplSourceFactory = Arc::new(move |_| {
            Arc::new(InProcessReplSource::new(leader_clone.clone())) as Arc<dyn ReplSource>
        });
        let supervisor =
            SubscriptionSupervisor::new(Arc::new(follower.clone()), factory, "follower")
                .with_poll_wait_ms(0)
                .with_max_iterations(1);
        supervisor.reconcile().await;
        supervisor.wait_for_subscription("sub").await;
        assert_admin_state(&follower, "resync_required").await;
        supervisor.reconcile().await;
        assert_eq!(
            supervisor.active_count(),
            0,
            "terminal refusal is not restarted"
        );
        assert_admin_state(&follower, "resync_required").await;
        assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
        assert_eq!(repo(&follower).current_commit_version().await.unwrap(), 0);
        assert!(rows(&follower).await.is_empty());
        supervisor.stop_all().await;
        // Apply the actual payload delivered alongside the explicit GAP, then
        // persist its leader bookmark to model a follower caught up through H.
        let seeded = open(None).await;
        let outcome = repo(&seeded)
            .apply_replicated(
                &events[0].event,
                &events[0].field_bindings,
                0,
                ApplyPolicy::default(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
        repo(&seeded).advance_replication_bookmark(h).await.unwrap();
        assert_eq!(repo(&seeded).replication_bookmark().await.unwrap(), h);
        assert_eq!(rows(&seeded).await, rows(&leader).await);

        // The source's certified gap ends at H; H+1 and later pulls are valid.
        let later = write(&leader, "after-gap").await;
        assert!(later > h);
        run(&seeded, &leader).await.unwrap();
        assert_eq!(repo(&seeded).replication_bookmark().await.unwrap(), later);
        assert_eq!(rows(&seeded).await, rows(&leader).await);

        // A fresh bookmark remains rejected despite the complete journal.
        let fresh = open(None).await;
        assert!(matches!(
            run(&fresh, &leader).await,
            Err(ReplError::JournalGap { .. }),
        ));
        assert!(rows(&fresh).await.is_empty());

        // A separately rebuilt lineage is gap-free and replays from zero.
        let rebuilt = open(None).await;
        let authoritative = rows(&leader).await;
        let mut import = Batch::new();
        import.transactional();
        import.insert("seed", insert("items").rows(authoritative.clone()));
        let response = rebuilt.execute("app", &import.build()).await.unwrap();
        assert!(!response.results.contains_key("__error"), "{response:?}");
        let rebuilt_head = repo(&rebuilt).current_commit_version().await.unwrap();
        let journal = repo(&rebuilt).read_changelog_bounded(1, 100).await.unwrap();
        assert!(journal.confirmed, "rebuilt journal must confirm its prefix");
        assert_eq!(journal.bound, rebuilt_head);
        let replay = open(None).await;
        run(&replay, &rebuilt).await.unwrap();
        assert_eq!(
            repo(&replay).replication_bookmark().await.unwrap(),
            rebuilt_head
        );
        assert_eq!(rows(&replay).await, authoritative);
    });
    drop(rt);
}

#[test]
fn clean_reopen_allows_follower_at_head() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("leader");
    let rt = runtime();
    let h = rt.block_on(async {
        let leader = open(Some(&path)).await;
        let h = write(&leader, "clean").await;
        repo(&leader)
            .shutdown_clean(Duration::from_secs(30))
            .await
            .unwrap();
        h
    });
    drop(rt);

    let rt = runtime();
    rt.block_on(async {
        let leader = open(Some(&path)).await;
        assert_eq!(
            repo(&leader)
                .read_changelog_from(1, 100)
                .await
                .unwrap()
                .gap_at,
            None
        );
        let follower = open(None).await;
        run(&follower, &leader).await.unwrap();
        assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), h);
        assert_eq!(rows(&follower).await, rows(&leader).await);
        let source = InProcessReplSource::new(leader.clone());
        let reply = source.pull("app", "main", h + 1, 100, None).await.unwrap();
        let ReplResponse::Pull { events, gap_at, .. } = reply else {
            panic!("expected head pull: {reply:?}");
        };
        assert_eq!(gap_at, None);
        let events: Vec<ReplicatedEvent> = rmp_serde::from_slice(&events).unwrap();
        assert!(events.is_empty(), "head pull must contain no events");
        assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), h);
    });
    drop(rt);
}
