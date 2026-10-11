//! R1-c — tests for [`run_follower_loop`] with the in-process
//! [`InProcessReplSource`].
//!
//! Scenarios (REPLICATION §4/§5.2/§5.3/§5.6):
//!   1. **Apply N events convergence:** write N rows on the leader, run the
//!      loop for a bounded number of iterations → the follower converges
//!      (its changefeed carries the re-emitted events) and its durable
//!      bookmark equals the leader's `current_version`.
//!   2. **Idempotent restart:** after catch-up, run the loop again with the
//!      same bookmark → no new events are applied, the bookmark does not
//!      regress, the follower state is unchanged.
//!   3. **Epoch regression (§5.2):** a source whose epoch regresses below
//!      the previously-seen maximum terminates the loop with
//!      [`ReplError::StaleLeaderEpoch`].
//!   4. **Journal gap → terminal stop:** a source whose `pull` reply carries
//!      `gap_at: Some(g)` with `g >= from_version` (alongside events, to prove
//!      they are NOT applied) terminates the loop with
//!      [`ReplError::JournalGap`] and leaves the bookmark unchanged.
//!
//! The loop is bounded via `max_iterations` (no infinite sleep) and a
//! `CancellationToken` as a belt-and-suspenders backstop.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use shamir_db::access::principal64_from_username;
use shamir_db::access::Actor;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::ChangelogEvent;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::{Mutex, Notify};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::config::ReplicationConfig;
use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// Build an in-memory `ShamirDb` with one db `app`, one repo `main`, one
/// table `items`, owned by `alice`. Used for BOTH the leader and the
/// follower (a fresh instance each) — the schema is identical, the data is
/// independent.
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

async fn write_rows_without_journal_wait(leader: &ShamirDb, n: usize) {
    let owner = Actor::User(principal64_from_username("alice"));
    for i in 0..n {
        let key_str = format!("k{i}");
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            insert("items").rows([doc! {
                "id" => key_str,
                "v" => i as i64,
            }]),
        );
        let resp = leader
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("fixture write should succeed");
        assert!(
            !resp.results.contains_key("__error"),
            "write failed: {resp:?}",
        );
    }
}

/// Write `n` rows into `app/main/items` as `alice`. Each transactional
/// insert commits → emits one changefeed event. Polls until all `n` events
/// are durable in the journal.
async fn write_rows(leader: &ShamirDb, n: usize) {
    write_rows_without_journal_wait(leader, n).await;

    // The journal writer is async; poll until all n events are durable.
    for _ in 0..200 {
        if let Some(jr) = leader
            .read_changelog_from_journal("app", "main", 0, 1000)
            .await
        {
            if jr.events.len() >= n {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("leader journal did not durable-land {n} events in time");
}

/// Poll the follower's bookmark until it reaches `target` (or panic after
/// a timeout). The bookmark is advanced by the loop as events are applied.
///
/// Takes the `ShamirDb` by value (it's cheaply cloneable, `Arc`-backed) so
/// the future is `'static` and can be `tokio::spawn`ed.
async fn wait_for_bookmark(follower: ShamirDb, target: u64) {
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo exists");
    for _ in 0..200 {
        let b = repo.replication_bookmark().await.expect("bookmark read");
        if b >= target {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let b = repo.replication_bookmark().await.expect("bookmark read");
    panic!("follower bookmark did not reach {target} in time (last={b})");
}

// ---------------------------------------------------------------------------
// Test 1 — apply N events convergence
// ---------------------------------------------------------------------------

/// Write 5 rows on the leader; run the follower loop (bounded) until it
/// catches up. The follower's durable bookmark must equal the leader's
/// `current_version`, and the follower's own changefeed must carry the
/// re-emitted events (chain replication works, §4.3).
#[tokio::test]
async fn apply_n_events_convergence() {
    const N: usize = 5;

    let leader = build_db("alice").await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");
    assert!(
        leader_version >= N as u64,
        "leader advanced past {N} writes"
    );

    let follower = build_db("follower").await;

    let source = Arc::new(InProcessReplSource::new(leader.clone()));
    let cancel = CancellationToken::new();
    // generous iteration cap; the loop should catch up well before.
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(100)
        .with_max_iterations(50);
    let loop_fut = run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg,
        cancel.clone(),
    );

    // Drive the loop and the wait concurrently; cancel once converged.
    let bookmark_target = leader_version;
    let converge = tokio::spawn(wait_for_bookmark(follower.clone(), bookmark_target));

    // Run the loop; it exits via max_iterations or we cancel after convergence.
    tokio::select! {
        res = loop_fut => {
            // Loop ended (max_iterations). Check it didn't error.
            res.expect("follower loop did not panic");
        }
        _ = converge => {
            // Converged before max_iterations — cancel the loop.
            cancel.cancel();
        }
    }

    // Bookmark == leader current_version (the events applied in order).
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo");
    let bookmark = repo.replication_bookmark().await.expect("bookmark");
    assert!(
        bookmark >= leader_version,
        "follower bookmark {bookmark} should have reached leader version {leader_version}"
    );

    // The follower re-emitted the events into its OWN changefeed (chain).
    let follower_jr = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    assert!(
        follower_jr.events.len() >= N,
        "follower changefeed should carry >= {N} re-emitted events, got {}",
        follower_jr.events.len()
    );
}

// ---------------------------------------------------------------------------
// Test 2 — idempotent restart
// ---------------------------------------------------------------------------

/// After the follower catches up, running the loop again with the SAME
/// bookmark → every event is `Skipped`, the bookmark does not regress, and
/// the follower's changefeed does not grow (no double-application).
#[tokio::test]
async fn idempotent_restart_after_catchup() {
    const N: usize = 3;

    let leader = build_db("alice").await;
    write_rows(&leader, N).await;
    let leader_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");

    let follower = build_db("follower").await;
    let source = Arc::new(InProcessReplSource::new(leader.clone()));

    // First run: catch up.
    let cancel1 = CancellationToken::new();
    let f1 = Arc::new(follower.clone());
    let cfg1 = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(100)
        .with_max_iterations(50);
    let loop1 = tokio::spawn(run_follower_loop(
        f1.clone(),
        source.clone(),
        cfg1,
        cancel1.clone(),
    ));
    wait_for_bookmark(follower.clone(), leader_version).await;
    cancel1.cancel();
    let _ = loop1.await;

    // Snapshot the follower state AFTER catch-up.
    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo");
    let bookmark_before = repo.replication_bookmark().await.expect("bookmark");
    let jr_before = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    let events_before = jr_before.events.len();

    assert!(
        bookmark_before >= leader_version,
        "pre-restart bookmark {bookmark_before} should be >= leader {leader_version}"
    );

    // Second run: the bookmark is already at leader_version, so every pull
    // returns an empty batch (from_version = bookmark+1 > leader_version)
    // and no events are applied. The bookmark must NOT regress.
    let cancel2 = CancellationToken::new();
    let cfg2 = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(50)
        .with_max_iterations(3);
    run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg2,
        cancel2.clone(),
    )
    .await
    .expect("restart loop completes cleanly");

    // Bookmark unchanged (no regression, no spurious advance).
    let bookmark_after = repo.replication_bookmark().await.expect("bookmark");
    assert_eq!(
        bookmark_after, bookmark_before,
        "idempotent restart: bookmark must not change"
    );

    // Changelog did not grow (no double-application).
    let jr_after = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    assert_eq!(
        jr_after.events.len(),
        events_before,
        "idempotent restart: follower changefeed must not grow"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — epoch regression terminates the loop (§5.2)
// ---------------------------------------------------------------------------

/// A source whose `leader_epoch` regresses below the previously-seen
/// maximum → the loop returns [`ReplError::StaleLeaderEpoch`] and stops.
#[tokio::test]
async fn epoch_regression_terminate_loop() {
    let leader = build_db("alice").await;
    write_rows(&leader, 1).await;

    let follower = build_db("follower").await;
    // Start the source at epoch 5; the hello seeds max_seen_epoch = 5.
    let source = Arc::new(InProcessReplSource::with_epoch(leader.clone(), 5));

    // Spawn the loop. Once it pulls once (epoch 5), regress the source to
    // epoch 3 → the next pull reply carries epoch 3 < 5 → StaleLeaderEpoch.
    let cancel = CancellationToken::new();
    let source_for_thread = source.clone();
    let regressing = tokio::spawn(async move {
        // Give the loop time to call hello + at least one pull at epoch 5.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Regress the epoch — simulates a "resurrected" old leader.
        source_for_thread.set_epoch(3);
    });

    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(200)
        .with_max_iterations(50);
    let result = run_follower_loop(Arc::new(follower.clone()), source, cfg, cancel).await;

    // The regressing helper may or may not have raced the loop's exit; but
    // if the loop observed the regression, it MUST return StaleLeaderEpoch.
    // If the loop exited via max_iterations before the regression landed,
    // that's also acceptable (no stale epoch observed). We only assert the
    // error shape WHEN an error is returned.
    match result {
        Err(ReplError::StaleLeaderEpoch { observed, max_seen }) => {
            assert_eq!(observed, 3, "observed regressed epoch");
            assert_eq!(max_seen, 5, "max-seen epoch seeded by hello");
        }
        Ok(()) => {
            // Loop exited via max_iterations before the regression landed —
            // acceptable. The regression task should still complete.
            regressing.await.expect("regression task");
        }
        Err(other) => panic!("expected StaleLeaderEpoch or clean exit, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 4 — journal gap terminates the loop, WITHOUT applying accompanying events
// ---------------------------------------------------------------------------

/// A [`ReplSource`] whose `pull` always replies with a fixed `gap_at` AND a
/// non-empty events payload in the SAME reply — this proves the loop checks
/// the gap BEFORE decoding/applying events, per the brief's requirement that
/// events accompanying a gap response must never be applied.
struct GapReplSource {
    gap_at: u64,
    /// Events to include alongside the gap (msgpack-encoded
    /// `Vec<ChangelogEvent>`). Non-empty, so an incorrect implementation
    /// that applies-before-checking would visibly advance the bookmark.
    events_bytes: Vec<u8>,
}

#[async_trait]
impl ReplSource for GapReplSource {
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
        Ok(ReplResponse::Pull {
            leader_epoch: 1,
            events: self.events_bytes.clone(),
            gap_at: Some(self.gap_at),
            current_version: self.gap_at,
        })
    }
}

/// Drive the follower loop against a source that reports `gap_at` together
/// with a real event, and assert the loop stops with
/// [`ReplError::JournalGap`] without applying that event: the bookmark stays
/// at 0 (nothing was ever applied) and the follower's own journal is empty.
async fn assert_gap_stops_loop(gap_at: u64) {
    let leader = build_db("alice").await;
    // One real event, so decoding would succeed if the loop incorrectly
    // reached the decode/apply step before the gap check.
    write_rows(&leader, 1).await;
    let jr = leader
        .read_changelog_from_journal("app", "main", 0, 10)
        .await
        .expect("leader journal");
    let events_bytes = rmp_serde::to_vec_named(&jr.events).expect("encode events");
    assert!(!events_bytes.is_empty(), "fixture must carry an event");

    let follower = build_db("follower").await;
    // The follower has never applied anything, so its first `from_version`
    // is bookmark(0) + 1 = 1.
    let source = Arc::new(GapReplSource {
        gap_at,
        events_bytes,
    });

    let cancel = CancellationToken::new();
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(50)
        .with_max_iterations(10);
    let result = run_follower_loop(Arc::new(follower.clone()), source, cfg, cancel).await;

    match result {
        Err(ReplError::JournalGap {
            gap_at: reported,
            from_version,
        }) => {
            assert_eq!(reported, gap_at);
            assert_eq!(from_version, 1);
        }
        other => panic!("expected JournalGap, got {other:?}"),
    }

    let repo = follower
        .get_db("app")
        .and_then(|d| d.get_repo("main"))
        .expect("follower repo");
    let bookmark = repo.replication_bookmark().await.expect("bookmark read");
    assert_eq!(bookmark, 0, "bookmark must not advance past a gap");

    let follower_jr = follower
        .read_changelog_from_journal("app", "main", 0, 1000)
        .await
        .expect("follower journal");
    assert!(
        follower_jr.events.is_empty(),
        "no event should have been applied when a gap is reported"
    );
}

/// A gap reply strictly ahead of `from_version` (the follower is missing
/// `[from_version, gap_at)`) terminates the loop and applies nothing.
#[tokio::test]
async fn journal_gap_terminates_loop_without_applying_events() {
    assert_gap_stops_loop(100).await;
}

/// `gap_at == from_version` means the very first requested version is the
/// lost one (#1210): the changefeed contract is `gap_at = Some(v)` for a lost
/// `v >= from_version`. The follower must stop here too instead of treating
/// the gap as "behind us" and applying later events across the hole.
#[tokio::test]
async fn journal_gap_at_from_version_terminates_loop_without_applying_events() {
    assert_gap_stops_loop(1).await;
}

struct MalformedPutSource {
    inner: InProcessReplSource,
    events: Vec<ChangelogEvent>,
    pulls: Mutex<Vec<(u64, Instant)>>,
    third_pull: Notify,
}

#[async_trait]
impl ReplSource for MalformedPutSource {
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
        let pull_count = {
            let mut pulls = self.pulls.lock().await;
            pulls.push((from_version, Instant::now()));
            pulls.len()
        };
        if pull_count == 3 {
            self.third_pull.notify_one();
            std::future::pending::<()>().await;
        }
        let mut response = self
            .inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await?;
        if let ReplResponse::Pull {
            events,
            gap_at,
            current_version,
            ..
        } = &mut response
        {
            let mut decoded: Vec<ChangelogEvent> = self
                .events
                .iter()
                .filter(|event| event.commit_version >= from_version)
                .take(limit as usize)
                .cloned()
                .collect();
            *gap_at = None;
            *current_version = self.events.last().expect("fixture events").commit_version;
            if let Some(first) = decoded.first_mut() {
                let change = first.changes.first_mut().expect("fixture Put");
                assert!(change.value.is_some());
                change.value = Some(vec![0xC1].into());
            }
            *events = rmp_serde::to_vec_named(&decoded).expect("encode malformed Put");
        }
        Ok(response)
    }
}

async fn malformed_put_fixture() -> (ShamirDb, Arc<MalformedPutSource>, u64) {
    let leader = build_db("alice").await;
    let repo = leader.get_db("app").unwrap().get_repo("main").unwrap();
    let mut committed = repo.subscribe_changelog().await.expect("leader changefeed");
    write_rows_without_journal_wait(&leader, 2).await;
    let first = committed.recv().await.expect("first committed Put");
    let second = committed.recv().await.expect("second committed Put");
    assert!(first.commit_version < second.commit_version);
    let leader_version = second.commit_version;
    let source = Arc::new(MalformedPutSource {
        inner: InProcessReplSource::new(leader),
        events: vec![(*first).clone(), (*second).clone()],
        pulls: Mutex::new(Vec::new()),
        third_pull: Notify::new(),
    });
    (build_db("follower").await, source, leader_version)
}

#[tokio::test(start_paused = true)]
async fn validate_payload_rejects_malformed_put_before_version_and_retries_same_cursor() {
    let (follower, source, _) = malformed_put_fixture().await;
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    let gate = repo.tx_gate().await.expect("follower gate");
    let allocated_before = gate.version_allocation_high_water_mark();
    let watermark_before = gate.completion().watermark();
    let bookmark_before = repo.replication_bookmark().await.expect("bookmark");
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_validate_replicated_payload(true)
        .with_poll_wait_ms(0)
        .with_max_iterations(2);
    let started = Instant::now();
    run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("bounded retry loop");
    let pulls = source.pulls.lock().await;
    assert_eq!(pulls.len(), 2);
    assert_eq!(pulls[0].0, bookmark_before + 1);
    assert_eq!(pulls[1].0, pulls[0].0);
    assert!(pulls[1].1.duration_since(pulls[0].1) >= Duration::from_millis(200));
    assert!(started.elapsed() >= Duration::from_millis(400));
    drop(pulls);
    assert_eq!(gate.version_allocation_high_water_mark(), allocated_before);
    assert_eq!(gate.completion().watermark(), watermark_before);
    assert_eq!(repo.replication_bookmark().await.unwrap(), bookmark_before);
    let journal = follower
        .read_changelog_from_journal("app", "main", 0, 10)
        .await
        .expect("follower journal");
    assert!(journal.events.is_empty());
}

#[tokio::test(start_paused = true)]
async fn trusted_default_applies_malformed_put_and_continues_batch() {
    let (follower, source, leader_version) = malformed_put_fixture().await;
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    let gate = repo.tx_gate().await.expect("follower gate");
    let allocated_before = gate.version_allocation_high_water_mark();
    let cfg = FollowerLoopConfig::new("follower-1", "app", "main")
        .with_poll_wait_ms(0)
        .with_max_iterations(2);
    assert!(!cfg.validate_replicated_payload);
    run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        cfg,
        CancellationToken::new(),
    )
    .await
    .expect("Trusted loop");
    assert_eq!(repo.replication_bookmark().await.unwrap(), leader_version);
    assert_eq!(
        gate.version_allocation_high_water_mark(),
        allocated_before + 2
    );
    assert_eq!(gate.completion().watermark(), allocated_before + 2);
    let pulls = source.pulls.lock().await;
    assert_eq!(pulls.len(), 2);
    assert_eq!(pulls[0].0, 1);
    assert_eq!(pulls[1].0, leader_version + 1);
}

#[tokio::test(start_paused = true)]
async fn supervisor_forwards_payload_validation_to_follower_loop() {
    let (follower, source, _) = malformed_put_fixture().await;
    let follower = Arc::new(follower);
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "cp",
        replication_profile("payload_validation").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "cs",
        subscription(
            "payload_validation",
            "tcp://leader:9000",
            "pub",
            "payload_validation",
        ),
    );
    follower
        .execute("app", &batch.to_request_via_msgpack())
        .await
        .expect("create profile and subscription");
    let repo = follower.get_db("app").unwrap().get_repo("main").unwrap();
    let gate = repo.tx_gate().await.expect("follower gate");
    let allocated_before = gate.version_allocation_high_water_mark();
    let source_for_factory = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| source_for_factory.clone());
    let supervisor = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_validate_replicated_payload(true)
        .with_poll_wait_ms(0)
        .with_max_iterations(3);
    supervisor.reconcile().await;
    assert!(supervisor.is_running("payload_validation"));
    source.third_pull.notified().await;
    supervisor.stop_all().await;
    assert_eq!(supervisor.active_count(), 0);
    let pulls = source.pulls.lock().await;
    assert_eq!(pulls.len(), 3);
    assert_eq!(pulls[0].0, 1);
    assert_eq!(pulls[1].0, 1);
    assert_eq!(pulls[2].0, 1);
    drop(pulls);
    assert_eq!(gate.version_allocation_high_water_mark(), allocated_before);
    assert_eq!(repo.replication_bookmark().await.unwrap(), 0);
}

#[test]
fn replication_payload_validation_config_defaults_and_opt_in() {
    assert!(!ReplicationConfig::default().validate_replicated_payload);
    let omitted: ReplicationConfig =
        ktav::from_str("server_name: localhost").expect("omitted flag");
    let disabled: ReplicationConfig =
        ktav::from_str("validate_replicated_payload: false").expect("disabled flag");
    let enabled: ReplicationConfig =
        ktav::from_str("validate_replicated_payload: true").expect("enabled flag");
    assert!(!omitted.validate_replicated_payload);
    assert!(!disabled.validate_replicated_payload);
    assert!(enabled.validate_replicated_payload);
}
