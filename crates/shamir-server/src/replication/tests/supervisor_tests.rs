//! 386-b — tests for [`SubscriptionSupervisor`] driven by the declarative
//! catalogue (386-a) with an in-process [`InProcessReplSource`] factory.
//!
//! Topology: a `leader` `Arc<ShamirDb>` and a `follower` `Arc<ShamirDb>`, each
//! with the same `app/main/items` schema. The profile + subscription are
//! created on the FOLLOWER via admin batches (built ONLY through the
//! `shamir_query_builder::ddl::replication::*` builders). The supervisor's
//! `ReplSourceFactory` returns an `InProcessReplSource` over the leader, so no
//! wire/SCRAM stack is exercised — just the catalogue → loop binding.
//!
//! Scenarios (brief §Тесты):
//!   1. **create → converge:** active subscription → `reconcile()` starts the
//!      loop → follower applies the leader's writes, bookmark grows.
//!   2. **pause:** `alter_subscription().pause()` → `reconcile()` stops the
//!      loop (no longer registered).
//!   3. **resume:** `alter_subscription().resume()` → `reconcile()` restarts
//!      the loop → catches up again.
//!   4. **drop:** `drop_subscription()` → `reconcile()` stops + deregisters.
//!
//! Loops are bounded via `with_max_iterations` (no infinite sleep).

use std::sync::Arc;

use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_subscription, repl_scope, replication_profile, subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Build an in-memory `ShamirDb` with db `app`, repo `main`, table `items`,
/// owned by `alice`. Used for both leader and follower (independent data).
async fn build_db() -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username("alice"));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    Arc::new(shamir)
}

/// Write `n` rows into `app/main/items` on the leader, then poll until all `n`
/// events are durable in the journal (the journal writer is async).
async fn write_rows(leader: &ShamirDb, base: usize, n: usize) {
    let owner = Actor::User(principal64_from_username("alice"));
    let before = leader
        .read_changelog_from_journal("app", "main", 0, 100_000)
        .await
        .map(|jr| jr.events.len())
        .unwrap_or(0);
    for i in base..(base + n) {
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            insert("items").rows([doc! {
                "id" => format!("k{i}"),
                "v" => i as i64,
            }]),
        );
        let resp = leader
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("fixture write");
        assert!(
            !resp.results.contains_key("__error"),
            "write failed: {resp:?}"
        );
    }
    let want = before + n;
    for _ in 0..200 {
        if let Some(jr) = leader
            .read_changelog_from_journal("app", "main", 0, 100_000)
            .await
        {
            if jr.events.len() >= want {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("leader journal did not land {want} events");
}

/// Create a replication profile with a single `pull` stream over `app/main`,
/// then a subscription bound to it — both on the FOLLOWER catalogue.
async fn create_profile_and_subscription(follower: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cp",
        replication_profile("prof_a").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_a"),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("create profile+subscription");
}

/// Run an `alter_subscription` terminal op on the follower catalogue.
async fn alter(follower: &ShamirDb, op: shamir_query_types::batch::BatchOp) {
    let mut b = Batch::new();
    b.id(1);
    b.op("as", op);
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("alter subscription");
}

/// Build a supervisor whose factory yields an `InProcessReplSource` over the
/// leader, with a bounded iteration cap and short poll budget.
fn supervisor(follower: Arc<ShamirDb>, leader: Arc<ShamirDb>) -> SubscriptionSupervisor {
    let factory: ReplSourceFactory = Arc::new(move |_sub: &Subscription| {
        Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
    });
    SubscriptionSupervisor::new(follower, factory, "follower-1")
        .with_poll_wait_ms(50)
        .with_max_iterations(50)
}

/// Poll the follower's `app/main` bookmark until it reaches `target`.
async fn wait_for_bookmark(follower: &ShamirDb, target: u64) {
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo");
    for _ in 0..300 {
        if repo.replication_bookmark().await.expect("bookmark") >= target {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let b = repo.replication_bookmark().await.expect("bookmark");
    panic!("follower bookmark did not reach {target} (last={b})");
}

async fn bookmark(follower: &ShamirDb) -> u64 {
    follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo")
        .replication_bookmark()
        .await
        .expect("bookmark")
}

// ---------------------------------------------------------------------------
// Test 1 — create → converge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_then_converge() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let leader_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;

    assert!(
        sup.is_running("sub_a"),
        "loop should be running after reconcile"
    );
    assert_eq!(sup.active_count(), 1);

    wait_for_bookmark(&follower, leader_version).await;
    assert!(bookmark(&follower).await >= leader_version);

    sup.stop_all().await;
}

// ---------------------------------------------------------------------------
// Test 2 — pause
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_stops_loop() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let v1 = leader.current_commit_version("app", "main").await.unwrap();

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;
    wait_for_bookmark(&follower, v1).await;

    // Pause → reconcile stops the loop.
    alter(&follower, alter_subscription("sub_a").pause().into()).await;
    sup.notify_changed().await;
    assert!(!sup.is_running("sub_a"), "paused loop must be deregistered");
    assert_eq!(sup.active_count(), 0);

    // New leader writes must NOT be applied while paused.
    let paused_at = bookmark(&follower).await;
    write_rows(&leader, 100, 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        bookmark(&follower).await,
        paused_at,
        "bookmark must not advance while paused"
    );

    sup.stop_all().await;
}

// ---------------------------------------------------------------------------
// Test 3 — resume
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_restarts_loop() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let v1 = leader.current_commit_version("app", "main").await.unwrap();

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;
    wait_for_bookmark(&follower, v1).await;

    alter(&follower, alter_subscription("sub_a").pause().into()).await;
    sup.notify_changed().await;
    assert!(!sup.is_running("sub_a"));

    // More leader writes while paused.
    write_rows(&leader, 100, 3).await;
    let v2 = leader.current_commit_version("app", "main").await.unwrap();

    // Resume → reconcile restarts the loop → catches up to v2.
    alter(&follower, alter_subscription("sub_a").resume().into()).await;
    sup.notify_changed().await;
    assert!(sup.is_running("sub_a"), "resumed loop must be registered");

    wait_for_bookmark(&follower, v2).await;
    assert!(bookmark(&follower).await >= v2);

    sup.stop_all().await;
}

// ---------------------------------------------------------------------------
// Test 4 — drop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_stops_and_deregisters() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let v1 = leader.current_commit_version("app", "main").await.unwrap();

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;
    wait_for_bookmark(&follower, v1).await;
    assert!(sup.is_running("sub_a"));

    // Drop → reconcile stops + removes from registry.
    alter(&follower, drop_subscription("sub_a")).await;
    sup.notify_changed().await;
    assert!(!sup.is_running("sub_a"), "dropped loop must be gone");
    assert_eq!(sup.active_count(), 0);

    sup.stop_all().await;
}
