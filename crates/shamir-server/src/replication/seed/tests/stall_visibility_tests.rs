//! PR6b-3 — stall visibility tests: a follower loop that swallows repeated
//! transient errors WITHOUT durable-bookmark progress must surface the stall
//! as a persistent `status_message` on the subscription row (never silent
//! again), while any bookmark progress resets the failure counter and can
//! never block normal convergence.
//!
//! Bound by construction: [`STALL_VISIBILITY_THRESHOLD`] is 10, strictly
//! greater than the engine's inline retry budget of 3, so the
//! `receipt_io_pause_retry_convergence` scenario (3 failures, then
//! convergence) can never trip it.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::access::principal64_from_username;
use shamir_db::access::Actor;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::filter::FilterContext;
use shamir_db::types::common::new_map;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::query::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

async fn build_db(owner_label: &str) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username(owner_label));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    shamir
}

/// Create the `system/subscriptions` row the loop marks on a stall.
async fn create_subscription_row(follower: &ShamirDb, name: &str) {
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "cp",
        replication_profile("stall_profile").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "cs",
        subscription(name, "tcp://leader:9000", "pub", "stall_profile"),
    );
    follower
        .execute("app", &batch.to_request_via_msgpack())
        .await
        .expect("create profile and subscription");
}

/// Read the single subscription row's `state` / `status_message`.
async fn read_subscription_row(follower: &ShamirDb) -> (Option<String>, Option<String>) {
    let table = follower.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query = Query::from("subscriptions").build();
    let rows = table.read(&query, &ctx).await.unwrap();
    let row = rows.records[0].as_value();
    (
        row.get("state").and_then(|v| v.as_str()).map(String::from),
        row.get("status_message")
            .and_then(|v| v.as_str())
            .map(String::from),
    )
}

/// A source whose `pull` (and initial `hello`) always fails transiently —
/// the exact silent-retry shape that motivated PR6b-3.
struct AlwaysFailingSource;

#[async_trait]
impl ReplSource for AlwaysFailingSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport("leader unreachable".into()))
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        Err(ReplError::Transport("leader unreachable".into()))
    }
}

/// Fails the first `failures` pulls, then delegates to a healthy
/// [`InProcessReplSource`] so the bookmark advances (progress).
struct FlakyThenHealthySource {
    remaining_failures: Mutex<u32>,
    inner: InProcessReplSource,
}

#[async_trait]
impl ReplSource for FlakyThenHealthySource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let mut remaining = self.remaining_failures.lock().await;
        if *remaining > 0 {
            *remaining -= 1;
            return Err(ReplError::Transport("transient flake".into()));
        }
        drop(remaining);
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

#[tokio::test(start_paused = true)]
async fn repeated_failures_without_progress_surface_status_message() {
    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    // Enough iterations to cross the threshold and keep retrying past it.
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD + 2);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(AlwaysFailingSource),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("swallowed transient errors must NOT terminate the loop");
    let (state, message) = read_subscription_row(&follower).await;
    // Least-destructive surfacing: the row stays active/retryable.
    assert_eq!(
        state.as_deref(),
        Some("active"),
        "stall must stay retryable"
    );
    let message = message.expect("stall must be visible via status_message");
    // R7-4: the assertion matches the production message in
    // follower_loop.rs::note_swallowed_failure EXACTLY — any wording drift
    // here must fail this test, not silently pass on a substring.
    assert_eq!(
        message,
        "follower stalled: 10 consecutive failures without progress; \
         last error: leader unreachable",
        "unexpected status_message: {message}"
    );
}

/// Fails the first 10 pulls ("streak one"), then delegates twice to the
/// healthy inner source (durable progress), then fails every further pull
/// ("streak two") — stall → recovery → fresh stall in ONE loop run.
struct StreakySource {
    inner: InProcessReplSource,
    pulls: Mutex<u32>,
}

#[async_trait]
impl ReplSource for StreakySource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let n = {
            let mut pulls = self.pulls.lock().await;
            *pulls += 1;
            *pulls
        };
        if n <= 10 {
            return Err(ReplError::Transport("streak one".into()));
        }
        if n <= 12 {
            return self
                .inner
                .pull(db, repo, from_version, limit, wait_ms)
                .await;
        }
        Err(ReplError::Transport("streak two".into()))
    }
}

/// R7-5: durable progress must OVERWRITE a reported stall message with the
/// progress note AND restart the failure counter, so a later fresh
/// 10-failure streak reports again with the fresh count.
///
/// COUNTERFACTUAL: if the counter were NOT restarted on progress, the
/// post-progress streak continues at 11 (counts 11..20 ≥ threshold 10 with
/// the latch cleared by `clear_stall`) and the message would read "…11
/// consecutive failures…" — the first failing assertion is the
/// exact-match `assert_eq!` on `10 consecutive failures` below. Without the
/// progress overwrite, the message could never have been the progress note
/// between the two streaks at all.
#[tokio::test(start_paused = true)]
async fn progress_overwrites_stall_message_and_counter_restarts() {
    let leader = build_db("alice").await;
    let owner = Actor::User(principal64_from_username("alice"));
    use shamir_query_builder::batch::Batch as B;
    use shamir_query_builder::doc;
    use shamir_query_builder::write::insert;
    let mut batch = B::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("items").rows([doc! {
            "id" => "k0",
            "v" => 0_i64,
        }]),
    );
    leader
        .execute_as(owner, "app", &batch.build())
        .await
        .expect("leader write");

    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    // 10 failures + 2 healthy pulls (one applied + one idle) + 10 failures.
    let source = StreakySource {
        inner: InProcessReplSource::new(leader),
        pulls: Mutex::new(0),
    };
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(25);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(source),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("stall/recovery/stall must never terminate the loop");
    // The healthy window durably applied the leader's event.
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 1, "converged");
    let (_, message) = read_subscription_row(&follower).await;
    // Fresh streak of exactly 10 → fresh count in the message.
    assert_eq!(
        message.as_deref(),
        Some(
            "follower stalled: 10 consecutive failures without progress; \
             last error: streak two"
        ),
        "the second streak must report with a RESTARTED counter: {message:?}"
    );
}

/// A source whose `pull` SUCCEEDS but returns an EMPTY batch while its
/// `current_version` advertises a backlog past the follower's bookmark —
/// the leader says "I have version 5" and hands over nothing, forever.
struct EmptyBacklogSource;

#[async_trait]
impl ReplSource for EmptyBacklogSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        // Transient hello: the loop seeds epoch 0 and proceeds to pulls.
        Err(ReplError::Transport("no hello".into()))
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
            leader_epoch: 1,
            events: Vec::new(),
            gap_at: None,
            current_version: 5,
            repo_lineage_id: Some([2; 16]),
            history_epoch: Some(0),
            gap_first: None,
            gap_through: None,
            schema_digest: None,
        })
    }
}

/// R7-5: successful empty pulls where the leader ADVERTISES a backlog
/// (`current_version` > durable bookmark) must feed the stall measure.
///
/// COUNTERFACTUAL: before this change a successful pull NEVER fed the stall
/// measure, so this loop ran forever with `state = active` and
/// `status_message = null` — the exact silent-green stall round 6 exposed in
/// CI. The first failing assertion is `message.expect(...)` below.
#[tokio::test(start_paused = true)]
async fn empty_pull_with_advertised_backlog_feeds_stall_measure() {
    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(crate::replication::follower_loop::STALL_VISIBILITY_THRESHOLD + 2);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(EmptyBacklogSource),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("successful-but-empty pulls must NOT terminate the loop");
    let (state, message) = read_subscription_row(&follower).await;
    assert_eq!(state.as_deref(), Some("active"), "stall stays retryable");
    let message = message.expect("backlog stall must be visible via status_message");
    assert_eq!(
        message,
        "follower stalled: 10 consecutive failures without progress; \
         last error: leader advertises backlog through version 5 but \
         returned no events from 1",
        "unexpected status_message: {message}"
    );
}

#[tokio::test(start_paused = true)]
async fn progress_resets_counter_and_normal_convergence_is_never_blocked() {
    let leader = build_db("alice").await;
    let owner = Actor::User(principal64_from_username("alice"));
    use shamir_query_builder::batch::Batch as B;
    use shamir_query_builder::doc;
    use shamir_query_builder::write::insert;
    let mut batch = B::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("items").rows([doc! {
            "id" => "k0",
            "v" => 0_i64,
        }]),
    );
    leader
        .execute_as(owner, "app", &batch.build())
        .await
        .expect("leader write");
    // Wait for the journal to land.
    for _ in 0..200 {
        if leader
            .read_changelog_from_journal("app", "main", 0, 10)
            .await
            .ok()
            .flatten()
            .is_some_and(|jr| !jr.events.is_empty())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let follower = build_db("follower").await;
    create_subscription_row(&follower, "stall_sub").await;
    // 3 failures (the engine's inline retry budget) then healthy progress —
    // the exact receipt_io_pause_retry_convergence shape. Must NOT trip.
    let source = FlakyThenHealthySource {
        remaining_failures: Mutex::new(3),
        inner: InProcessReplSource::new(leader),
    };
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_subscription("stall_sub")
        .with_max_iterations(10);
    run_follower_loop(
        Arc::new(follower.clone()),
        Arc::new(source),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("3 failures + convergence must converge cleanly");
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 1, "converged");
    let (state, message) = read_subscription_row(&follower).await;
    assert_eq!(state.as_deref(), Some("active"));
    assert!(
        message.is_none(),
        "progress must reset the counter — no stall message may exist: {message:?}"
    );
}
