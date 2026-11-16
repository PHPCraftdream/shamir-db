use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Barrier;
use std::thread;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};
use shamir_server_security::observability::StreamState;

use super::super::metrics::{ReplicationRegistry, StreamMetrics};

#[test]
fn concurrent_registration_reuses_winner_and_bounds_overflow() {
    let registry = Arc::new(ReplicationRegistry::new(1));
    let barrier = Arc::new(Barrier::new(13));
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                registry.register("app", "main").0
            })
        })
        .collect();
    barrier.wait();
    let mut registered = threads
        .into_iter()
        .map(|thread| thread.join().expect("registration thread"));
    let winner = registered.next().expect("at least one registration");
    for candidate in registered {
        assert!(Arc::ptr_eq(&winner, &candidate));
    }

    let overflow_a = registry.register("app", "archive");
    let overflow_b = registry.register("analytics", "main");
    assert!(overflow_a.1 && overflow_b.1);
    assert!(!Arc::ptr_eq(&overflow_a.0, &overflow_b.0));
    assert_eq!(registry.registered_count(), 1);
    let rows = registry.snapshot_for_test();
    assert_eq!(rows.len(), 2, "one named cell plus the bounded aggregate");
    assert!(rows
        .iter()
        .any(|row| row.db == "(other)" && row.repo == "(other)"));
}

#[test]
fn named_lag_tracks_current_value_including_zero() {
    let registry = ReplicationRegistry::new(1);
    let stream = StreamMetrics::start("app", "main", Some(&registry));
    stream.set_lag(10);
    stream.set_lag(0);
    let row = registry
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "app" && row.repo == "main")
        .expect("named follower row");
    assert_eq!(row.lag_versions, 0);
}

#[test]
fn overflow_lag_aggregates_active_current_values() {
    let registry = ReplicationRegistry::new(0);
    let first = StreamMetrics::start("app", "one", Some(&registry));
    let second = StreamMetrics::start("app", "two", Some(&registry));
    first.set_lag(10);
    second.set_lag(7);
    assert_eq!(registry.snapshot_for_test()[0].lag_versions, 10);

    first.set_lag(0);
    assert_eq!(registry.snapshot_for_test()[0].lag_versions, 7);
    second.set_lag(0);
    assert_eq!(registry.snapshot_for_test()[0].lag_versions, 0);
}

#[test]
fn expired_overflow_stream_keeps_zeroed_other_row() {
    let registry = ReplicationRegistry::new(0);
    {
        let stream = StreamMetrics::start("app", "one", Some(&registry));
        stream.set_lag(10);
        assert_eq!(registry.snapshot_for_test()[0].lag_versions, 10);
    }
    let row = registry
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "(other)" && row.repo == "(other)")
        .expect("overflow aggregate row");
    assert_eq!(row.lag_versions, 0);
    assert_eq!(row.state_code, StreamState::Stopped.code());
}

#[test]
fn active_role_current_lag_wins_over_stopped_role_value() {
    let registry = ReplicationRegistry::new(1);
    {
        let follower = StreamMetrics::start("app", "main", Some(&registry));
        follower.set_lag(10);
    }
    let leader = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);
    leader.observe_pull(10, 10);
    let row = registry.snapshot_for_test().pop().expect("named row");
    assert_eq!(row.lag_versions, 0);

    drop(leader);
    let row = registry
        .snapshot_for_test()
        .pop()
        .expect("stopped named row");
    assert_eq!(row.lag_versions, 0);
}

#[test]
fn leader_receipt_requires_bookmark_advance_and_state_is_request_scoped() {
    let registry = ReplicationRegistry::new(1);
    let first = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);
    let second = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);
    first.observe_pull(10, 5);
    let row = registry.snapshot_for_test().pop().expect("named row");
    assert_eq!(row.lag_versions, 5);
    assert_eq!(
        row.last_receipt_ns, 0,
        "first bookmark establishes baseline"
    );

    second.observe_pull(12, 7);
    let receipt = registry.snapshot_for_test()[0].last_receipt_ns;
    assert!(receipt > 0, "advanced acknowledged bookmark sets receipt");
    second.observe_pull(12, 7);
    assert_eq!(registry.snapshot_for_test()[0].last_receipt_ns, receipt);

    drop(first);
    assert_eq!(
        registry.snapshot_for_test()[0].state_code,
        StreamState::Streaming.code()
    );
    drop(second);
    let row = registry
        .snapshot_for_test()
        .pop()
        .expect("stopped named row");
    assert_eq!(row.state_code, StreamState::Stopped.code());
    assert_eq!(row.lag_versions, 0, "completed request lag is not retained");
    assert_eq!(
        row.last_receipt_ns, receipt,
        "acknowledgment history persists"
    );
}

#[test]
fn overlapping_named_leader_requests_aggregate_only_live_current_lag() {
    let registry = ReplicationRegistry::new(1);
    let first = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);
    let second = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);

    first.observe_pull(100, 0);
    second.observe_pull(100, 100);
    assert_eq!(registry.snapshot_for_test()[0].lag_versions, 100);

    drop(first);
    assert_eq!(registry.snapshot_for_test()[0].lag_versions, 0);
    drop(second);
    assert_eq!(registry.snapshot_for_test()[0].lag_versions, 0);
}

#[test]
fn overflow_leader_requests_do_not_retain_acknowledgment_history() {
    let registry = ReplicationRegistry::new(0);
    let leader = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);
    leader.observe_pull(100, 0);
    leader.observe_pull(100, 100);
    assert_eq!(registry.snapshot_for_test()[0].last_receipt_ns, 0);

    drop(leader);
    let row = registry.snapshot_for_test().pop().expect("overflow row");
    assert_eq!(row.lag_versions, 0);
    assert_eq!(row.last_receipt_ns, 0);

    let next = super::super::metrics::LeaderStreamMetrics::new_for_test("app", "main", &registry);
    next.observe_pull(100, 100);
    let row = registry.snapshot_for_test().pop().expect("overflow row");
    assert_eq!(row.lag_versions, 0);
    assert_eq!(row.last_receipt_ns, 0);
}

#[test]
fn owner_updates_keep_exact_counter_and_durable_receipt_values() {
    let registry = ReplicationRegistry::new(2);
    let stream = StreamMetrics::start("app", "main", Some(&registry));
    stream.set_state(StreamState::Streaming);
    stream.set_lag(7);
    registry.note_source_created("app", "main", true);
    registry.note_source_created("app", "main", true);
    stream.note_durable_progress(3, 42);

    let row = registry
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "app" && row.repo == "main")
        .expect("registered follower row");
    assert_eq!(row.lag_versions, 3);
    assert_eq!(row.last_receipt_ns, 42);
    assert_eq!(row.state_code, StreamState::Streaming.code());
    assert_eq!(row.reconnects, 2);
}

async fn follower_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("init db");
    let owner = Actor::User(principal64_from_username("metrics-test"));
    db.create_db_as("metrics", owner.clone())
        .await
        .expect("create db");
    let repo =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    db.add_repo_as("metrics", repo, owner)
        .await
        .expect("create repo");
    db
}

struct IdleSource;

#[async_trait]
impl ReplSource for IdleSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport("initial hello unavailable".into()))
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        Ok(ReplResponse::Pull {
            schema_digest: None,
            repo_lineage_id: Some([1; 16]),
            history_epoch: Some(0),
            gap_first: None,
            gap_through: None,
            leader_epoch: 1,
            events: rmp_serde::to_vec_named(&Vec::<shamir_db::engine::tx::ReplicatedEvent>::new())
                .expect("encode empty event list"),
            gap_at: None,
            current_version: 9,
        })
    }
}

struct RetryThenDecodeErrorSource {
    pulls: AtomicUsize,
}

#[async_trait]
impl ReplSource for RetryThenDecodeErrorSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Ok(ReplResponse::Hello {
            leader_epoch: 1,
            repos: Vec::new(),
        })
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        match self.pulls.fetch_add(1, Ordering::Relaxed) {
            0 => Err(ReplError::Transport("temporary pull failure".into())),
            1 => Ok(ReplResponse::Pull {
                schema_digest: None,
                repo_lineage_id: Some([1; 16]),
                history_epoch: Some(0),
                gap_first: None,
                gap_through: None,
                leader_epoch: 1,
                events: vec![0xc1], // Invalid MessagePack: decode errors are not reconnects.
                gap_at: None,
                current_version: 0,
            }),
            _ => Ok(ReplResponse::Pull {
                schema_digest: None,
                repo_lineage_id: Some([1; 16]),
                history_epoch: Some(0),
                gap_first: None,
                gap_through: None,
                leader_epoch: 1,
                events: rmp_serde::to_vec_named(
                    &Vec::<shamir_db::engine::tx::ReplicatedEvent>::new(),
                )
                .expect("encode empty event list"),
                gap_at: None,
                current_version: 0,
            }),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn transport_and_decode_retries_do_not_count_as_reconnects() {
    let follower = Arc::new(follower_db().await);
    let registry = Arc::new(ReplicationRegistry::new(4));
    let config = FollowerLoopConfig::new("metrics-follower", "metrics", "main")
        .with_identity_fence(false)
        .with_poll_wait_ms(0)
        .with_max_iterations(3)
        .with_metrics_registry(Arc::clone(&registry));

    run_follower_loop(
        follower,
        Arc::new(RetryThenDecodeErrorSource {
            pulls: AtomicUsize::new(0),
        }),
        config,
        CancellationToken::new(),
    )
    .await
    .expect("transient and decode failures are retried");

    let row = registry
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "metrics" && row.repo == "main")
        .expect("follower metrics row");
    assert_eq!(
        row.reconnects, 0,
        "transport retries do not count as source recreations"
    );
    assert_eq!(row.last_receipt_ns, 0, "no durable apply occurred");
    assert_eq!(row.state_code, StreamState::Stopped.code());
}

#[tokio::test]
async fn real_follower_apply_records_durable_receipt_and_converges_metrics() {
    let leader = follower_db().await;
    let follower = Arc::new(follower_db().await);
    let owner = Actor::User(principal64_from_username("metrics-test"));
    let mut batch = Batch::named("metrics-write");
    batch.id("metrics-write");
    batch.transactional();
    batch.insert(
        "i",
        insert("items").rows([doc! {
            "id" => "receipt-event",
            "value" => 1i64,
        }]),
    );
    let response = leader
        .execute_as(owner, "metrics", &batch.build())
        .await
        .expect("leader write succeeds");
    assert!(
        !response.results.contains_key("__error"),
        "leader write failed: {response:?}"
    );

    let source = Arc::new(InProcessReplSource::new(leader.clone()));
    // This bounded long-poll waits for journal readiness and fails if no
    // event arrives within its budget.
    let barrier = source
        .pull("metrics", "main", 1, 1000, Some(5_000))
        .await
        .expect("leader journal pull");
    let ReplResponse::Pull { events, .. } = barrier else {
        panic!("expected leader pull reply, got {barrier:?}");
    };
    let decoded: Vec<shamir_db::engine::tx::ReplicatedEvent> =
        rmp_serde::from_slice(&events).expect("decode committed leader event");
    assert!(
        !decoded.is_empty(),
        "leader pull must contain the committed write"
    );

    let source_watermark = leader
        .current_commit_version("metrics", "main")
        .await
        .expect("leader source watermark");

    let registry = Arc::new(ReplicationRegistry::new(4));
    let config = FollowerLoopConfig::new("metrics-follower", "metrics", "main")
        .with_identity_fence(false)
        .with_poll_wait_ms(0)
        .with_max_iterations(1)
        .with_metrics_registry(Arc::clone(&registry));
    run_follower_loop(
        Arc::clone(&follower),
        source,
        config,
        CancellationToken::new(),
    )
    .await
    .expect("one real apply completes");

    let repo = follower
        .get_db("metrics")
        .and_then(|db| db.get_repo("main"))
        .expect("follower repo");
    let bookmark = repo.replication_bookmark().await.expect("durable bookmark");
    assert_eq!(bookmark, source_watermark);

    let row = registry
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "metrics" && row.repo == "main")
        .expect("follower metrics row");
    assert_eq!(row.lag_versions, 0);
    assert!(
        row.last_receipt_ns > 0,
        "durable apply records receipt time"
    );
    assert_eq!(row.state_code, StreamState::Stopped.code());
    assert_eq!(row.reconnects, 0, "healthy initial contact is not a retry");
}

#[test]
fn explicit_source_lifecycle_counts_named_and_overflow_reconnects() {
    let named = ReplicationRegistry::new(1);
    named.note_source_created("app", "main", false);
    named.note_source_created("app", "main", true);
    let named_row = named
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "app" && row.repo == "main")
        .expect("named follower row");
    assert_eq!(named_row.reconnects, 1);

    let overflow = ReplicationRegistry::new(0);
    overflow.note_source_created("app", "main", false);
    assert_eq!(
        overflow
            .snapshot_for_test()
            .iter()
            .map(|row| row.reconnects)
            .sum::<u64>(),
        0
    );
    overflow.note_source_created("app", "main", true);
    let overflow_row = overflow
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "(other)" && row.repo == "(other)")
        .expect("overflow reconnect aggregate");
    assert_eq!(overflow_row.reconnects, 1);
}

#[tokio::test]
async fn real_follower_pull_updates_lag_without_fabricating_receipt() {
    let follower = Arc::new(follower_db().await);
    let registry = Arc::new(ReplicationRegistry::new(4));
    let config = FollowerLoopConfig::new("metrics-follower", "metrics", "main")
        .with_identity_fence(false)
        .with_max_iterations(1)
        .with_metrics_registry(Arc::clone(&registry));

    run_follower_loop(
        follower,
        Arc::new(IdleSource),
        config,
        CancellationToken::new(),
    )
    .await
    .expect("one idle pull completes");

    let row = registry
        .snapshot_for_test()
        .into_iter()
        .find(|row| row.db == "metrics" && row.repo == "main")
        .expect("follower metrics row");
    assert_eq!(row.lag_versions, 9);
    assert_eq!(
        row.last_receipt_ns, 0,
        "idle pull cannot count as a receipt"
    );
    assert_eq!(row.state_code, StreamState::Stopped.code());
    assert_eq!(row.reconnects, 0, "failed initial Hello is not a reconnect");
}

async fn source_recreation_db(db_name: &str, repo_name: &str) -> Arc<ShamirDb> {
    let db = ShamirDb::init_memory()
        .await
        .expect("init source recreation db");
    let owner = Actor::User(principal64_from_username("source-recreation-test"));
    db.create_db_as(db_name, owner.clone())
        .await
        .expect("create source recreation db");
    let repo = RepoConfig::new(repo_name, BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("items"));
    db.add_repo_as(db_name, repo, owner)
        .await
        .expect("create source recreation repo");
    Arc::new(db)
}

#[tokio::test]
async fn supervisor_source_recreation_counts_only_factory_restarts() {
    const DB: &str = "owner_metrics_source_recreation_db";
    const REPO: &str = "owner_metrics_source_recreation_repo";
    const PROFILE: &str = "owner_metrics_source_recreation_profile";
    const SUBSCRIPTION: &str = "owner_metrics_source_recreation_subscription";

    let leader = source_recreation_db(DB, REPO).await;
    let follower = source_recreation_db(DB, REPO).await;
    let mut catalogue = Batch::new();
    catalogue.id(1);
    catalogue.op(
        "profile",
        replication_profile(PROFILE).stream(
            repl_scope(DB).repo(REPO).build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    catalogue.op(
        "subscription",
        subscription(
            SUBSCRIPTION,
            "tcp://source-recreation-leader:9000",
            "owner_metrics_source_recreation_publication",
            PROFILE,
        ),
    );
    follower
        .execute(
            DB,
            &catalogue
                .to_request_via_msgpack()
                .expect("encode source recreation catalogue"),
        )
        .await
        .expect("create source recreation profile and subscription");

    let factory_calls = Arc::new(AtomicUsize::new(0));
    let factory: ReplSourceFactory = {
        let leader = Arc::clone(&leader);
        let factory_calls = Arc::clone(&factory_calls);
        Arc::new(move |_subscription: &Subscription| {
            factory_calls.fetch_add(1, Ordering::AcqRel);
            Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
        })
    };
    let supervisor = SubscriptionSupervisor::new(follower, factory, "source-recreation-follower")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);

    supervisor.reconcile().await;
    supervisor.wait_for_subscription(SUBSCRIPTION).await;
    assert_eq!(factory_calls.load(Ordering::Acquire), 1);
    let initial_row = crate::replication::replication_metrics_snapshot()
        .into_iter()
        .find(|row| row.db == DB && row.repo == REPO)
        .expect("unique source recreation metrics row after initial start");
    assert_eq!(
        initial_row.reconnects, 0,
        "initial source is not a reconnect"
    );

    // The active subscription remains in the catalogue after its idle loop
    // exits, so reconciliation creates a second source rather than retrying
    // a failed Hello or Pull on the first source.
    supervisor.reconcile().await;
    supervisor.wait_for_subscription(SUBSCRIPTION).await;
    assert_eq!(factory_calls.load(Ordering::Acquire), 2);

    let row = crate::replication::replication_metrics_snapshot()
        .into_iter()
        .find(|row| row.db == DB && row.repo == REPO)
        .expect("unique source recreation metrics row");
    assert_eq!(row.reconnects, 1, "one source recreation is one reconnect");
    supervisor.stop_all().await;
}
