use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::source::ReplSource;
use shamir_server_security::observability::StreamState;

use super::super::metrics::{ReplicationRegistry, StreamMetrics};

#[test]
fn concurrent_registration_reuses_winner_and_bounds_overflow() {
    let registry = Arc::new(ReplicationRegistry::new(1));
    let initial = registry.register("app", "main").0;
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let registry = Arc::clone(&registry);
            thread::spawn(move || registry.register("app", "main").0)
        })
        .collect();
    for thread in threads {
        let registered = thread.join().expect("registration thread");
        assert!(Arc::ptr_eq(&initial, &registered));
    }

    let overflow_a = registry.register("app", "archive");
    let overflow_b = registry.register("analytics", "main");
    assert!(overflow_a.1 && overflow_b.1);
    assert!(Arc::ptr_eq(&overflow_a.0, &overflow_b.0));
    assert_eq!(registry.registered_count(), 1);
    let rows = registry.snapshots();
    assert_eq!(rows.len(), 2, "one named cell plus the bounded aggregate");
    assert!(rows.iter().any(|row| row.db == "(other)" && row.repo == "(other)"));
}

#[test]
fn owner_updates_keep_exact_counter_and_durable_receipt_values() {
    let registry = ReplicationRegistry::new(2);
    let stream = StreamMetrics::start("app", "main", Some(&registry));
    stream.set_state(StreamState::Streaming);
    stream.set_lag(7);
    stream.note_reconnect_attempt();
    stream.note_reconnect_attempt();
    stream.note_durable_progress(3, 42);

    let row = registry
        .snapshots()
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
    let repo = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("items"));
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
        .snapshots()
        .into_iter()
        .find(|row| row.db == "metrics" && row.repo == "main")
        .expect("follower metrics row");
    assert_eq!(row.lag_versions, 9);
    assert_eq!(row.last_receipt_ns, 0, "idle pull cannot count as a receipt");
    assert_eq!(row.state_code, StreamState::Stopped.code());
    assert_eq!(row.reconnects, 1, "first pull follows the failed Hello");
}
