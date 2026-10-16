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
//!   5. **journal gap → resync_required:** a source that always replies with
//!      a `gap_at` past the requested `from_version` → the follower loop
//!      terminates with `JournalGap` → the supervisor persists
//!      `state = "resync_required"` on the subscription's row → a subsequent
//!      `reconcile()` does NOT restart it (mirrors how `"paused"` behaves).
//!
//! Loops are bounded via `with_max_iterations` (no infinite sleep).

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::Stream;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::meta::repl::bookmark_v3::{save_bookmark_v3, BookmarkV3};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::table_manager::table_token_for;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::ReplicatedEvent;
use shamir_db::storage::error::{DbError, DbResult};
use shamir_db::storage::storage_in_memory::InMemoryStore;
use shamir_db::storage::types::{RecordKey, Store};
use shamir_db::types::record_id::RecordId;
use shamir_db::types::value::InnerValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_subscription, repl_scope, replication_profile, subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use shamir_tx::{ChangeOp, ChangelogEvent, MvccStore, RecordChange};

use crate::replication::error::ReplError;
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
        .ok()
        .flatten()
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
            .ok()
            .flatten()
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
    alter(
        &follower,
        alter_subscription("sub_a").pause().build().unwrap(),
    )
    .await;
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

    alter(
        &follower,
        alter_subscription("sub_a").pause().build().unwrap(),
    )
    .await;
    sup.notify_changed().await;
    assert!(!sup.is_running("sub_a"));

    // More leader writes while paused.
    write_rows(&leader, 100, 3).await;
    let v2 = leader.current_commit_version("app", "main").await.unwrap();

    // Resume → reconcile restarts the loop → catches up to v2.
    alter(
        &follower,
        alter_subscription("sub_a").resume().build().unwrap(),
    )
    .await;
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

// ---------------------------------------------------------------------------
// Test 5 — journal gap → resync_required (RI-10, Part 2)
// ---------------------------------------------------------------------------

/// A [`ReplSource`] whose `pull` always replies with a fixed `gap_at` well
/// past any requested `from_version` — every pull call terminates the
/// follower loop with [`ReplError::JournalGap`].
struct AlwaysGapReplSource {
    gap_at: u64,
}

#[async_trait]
impl ReplSource for AlwaysGapReplSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        // Exercise first-Pull adoption: no successful Hello in this fixture.
        Err(ReplError::Transport("fixture starts with Pull".into()))
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
            events: Vec::new(),
            gap_at: Some(self.gap_at),
            current_version: self.gap_at,
        })
    }
}

/// After a follower loop terminates with `JournalGap`, the supervisor
/// persists `state = "resync_required"` on the subscription's row, AND a
/// subsequent `reconcile()` does NOT restart it — mirroring how a `"paused"`
/// subscription already stays stopped across reconcile ticks (Test 2).
#[tokio::test]
async fn journal_gap_marks_resync_required_and_stays_stopped() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Factory always returns a source whose `pull` reports a gap at 100
    // (well past `bookmark(0) + 1 = 1`) — every loop iteration hits the
    // terminal `JournalGap` path immediately.
    let factory: ReplSourceFactory = Arc::new(move |_sub: &Subscription| {
        Arc::new(AlwaysGapReplSource { gap_at: 100 }) as Arc<dyn ReplSource>
    });
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(20)
        .with_max_iterations(50);

    sup.reconcile().await;

    // Wait for the spawned loop task to observe the gap and for the
    // supervisor to persist `resync_required` on the subscription row.
    let mut state = None;
    for _ in 0..200 {
        state = sub_state(&follower, "sub_a").await;
        if state.as_deref() == Some("resync_required") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        state.as_deref(),
        Some("resync_required"),
        "subscription row must be marked resync_required after a journal gap"
    );

    // The dead loop task leaves a stale registry entry (pre-existing
    // loop-liveness gap, out of scope for this task) — but a fresh
    // `reconcile()` must NOT start a new loop for a subscription whose row
    // is `resync_required` (only `active` rows are (re)started).
    sup.reconcile().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    state = sub_state(&follower, "sub_a").await;
    assert_eq!(
        state.as_deref(),
        Some("resync_required"),
        "reconcile() must not flip resync_required back to active on its own"
    );

    sup.stop_all().await;
}

// Red before FOLLOWER-DURABILITY: both subscriptions start destination loops.
#[tokio::test]
async fn duplicate_destination_refuses_second_source() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let mut batch = Batch::new();
    batch.id(2);
    batch.op(
        "second",
        subscription("sub_b", "tcp://other:9000", "pub_b", "prof_a"),
    );
    follower
        .execute("app", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let factory: ReplSourceFactory = Arc::new(move |_| {
        count.fetch_add(1, Ordering::AcqRel);
        Arc::new(AlwaysGapReplSource { gap_at: 100 }) as Arc<dyn ReplSource>
    });
    let sup =
        SubscriptionSupervisor::new(follower.clone(), factory, "follower-1").with_max_iterations(0);
    sup.reconcile().await;
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(sup.active_count(), 1);
    let a = sub_state(&follower, "sub_a").await;
    let b = sub_state(&follower, "sub_b").await;
    assert!(a.as_deref() == Some("duplicate_owner") || b.as_deref() == Some("duplicate_owner"));
    sup.stop_all().await;
}

// Red before fix: reconcile starts a loop despite the destination dead gate.
#[tokio::test]
async fn reopen_required_is_visible_and_resume_refused() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let destination = follower.get_db("app").unwrap().get_repo("main").unwrap();
    destination.tx_gate().await.unwrap().set_requires_reopen();
    let factory: ReplSourceFactory =
        Arc::new(|_| panic!("must refuse destination before constructing source"));
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("reopen_required"),
    );
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "resume",
        alter_subscription("sub_a").resume().build().unwrap(),
    );
    let error = follower
        .execute("app", &batch.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert!(
        format!("{error:?}").contains("reopen_required"),
        "{error:?}"
    );
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));
    sup.stop_all().await;
}

// ---------------------------------------------------------------------------
// S5c-X PR6b-3 — receipt I/O pause/retry convergence (replaces the retired
// partial-history fixture: ordinary history I/O failure does NOT reopen;
// the receipt pipeline retries transient `DbError::Storage` failures).
// ---------------------------------------------------------------------------

/// A wrapped MVCC history store whose `get` fails EXACTLY ONCE with a
/// transient `DbError::Storage`, then delegates — the same store-swap
/// technique the retired fixture used, but on the receipt pipeline's READ
/// path (counter pre-state / `get_current_many` cold fallback).
struct TransientReadHistory {
    inner: Arc<dyn Store>,
    failures: AtomicUsize,
}

#[async_trait]
impl Store for TransientReadHistory {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        if self.failures.fetch_add(1, Ordering::AcqRel) == 0 {
            return Err(DbError::Storage(
                "injected transient receipt-path read failure".into(),
            ));
        }
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    fn iter_stream(&self, size: usize) -> HistoryStream {
        self.inner.iter_stream(size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, size: usize) -> HistoryStream {
        self.inner.scan_prefix_stream(prefix, size)
    }

    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> HistoryStream {
        self.inner.iter_range_stream(start, end, size)
    }

    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> HistoryStream {
        self.inner.iter_range_stream_reverse(start, end, size)
    }
}

/// Swap `items`' MVCC store for one backed by `history` (pre-attach first so
/// the swap is the registration, mirroring the retired fixture's technique).
async fn swap_items_mvcc_history(follower: &ShamirDb, history: Arc<TransientReadHistory>) {
    let destination = follower.get_db("app").unwrap().get_repo("main").unwrap();
    let gate = destination.tx_gate().await.unwrap();
    destination.get_table("items").await.unwrap();
    let token = table_token_for("items");
    destination.per_table_mvcc().remove_sync(&token);
    assert!(destination
        .per_table_mvcc()
        .insert_sync(token, Arc::new(MvccStore::new(history, gate)))
        .is_ok());
}

/// One leader-originated event (insert of row `k1` at commit_version 1).
fn single_insert_event() -> Vec<ReplicatedEvent> {
    vec![ReplicatedEvent {
        event: ChangelogEvent {
            repo: "main".into(),
            commit_version: 1,
            tx_id: 1,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: RecordId([1u8; 16]).to_bytes(),
                op: ChangeOp::Put,
                value: Some(InnerValue::Str("row".into()).to_bytes().unwrap()),
            }],
        },
        field_bindings: Vec::new(),
    }]
}

async fn items_row_counter(follower: &ShamirDb) -> u64 {
    follower
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .get_table("items")
        .await
        .unwrap()
        .counter_for_test()
        .get()
        .await
        .unwrap()
}

/// Poll until the subscription row reaches `expected` (terminal-state
/// persistence is async — the loop task reports it).
async fn wait_for_sub_state(follower: &ShamirDb, expected: &str) {
    for _ in 0..200 {
        if sub_state(follower, "sub_a").await.as_deref() == Some(expected) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "subscription never reached state {expected} (last={:?})",
        sub_state(follower, "sub_a").await
    );
}

#[tokio::test]
async fn receipt_io_pause_retry_convergence() {
    let leader = build_db().await;
    write_rows(&leader, 0, 1).await;
    let leader_version = leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version");

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let history = Arc::new(TransientReadHistory {
        inner: Arc::new(InMemoryStore::new()),
        failures: AtomicUsize::new(0),
    });
    swap_items_mvcc_history(&follower, history.clone()).await;

    let events = single_insert_event();
    let source = Arc::new(PartialSource {
        events: rmp_serde::to_vec_named(&events).unwrap(),
        pulls: AtomicUsize::new(0),
    });
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(20)
        .with_max_iterations(50);
    sup.reconcile().await;

    // The transient failure is retried away and the batch CONVERGES.
    wait_for_bookmark(&follower, leader_version).await;
    assert_eq!(
        bookmark(&follower).await,
        leader_version,
        "bookmark advanced once to the leader version"
    );
    assert_eq!(
        history.failures.load(Ordering::Acquire),
        1,
        "the injected failure happened exactly once"
    );
    assert!(
        source.pulls.load(Ordering::Acquire) >= 2,
        "the pipeline re-pulled after the transient failure"
    );
    // One local version per event — no replacement allocations.
    let gate = follower
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    assert_eq!(gate.version_allocation_high_water_mark(), 1);
    assert_eq!(
        items_row_counter(&follower).await,
        1,
        "exactly one local row allocated — no replacements"
    );
    // A transient failure is NOT terminal: the subscription keeps running.
    assert!(sup.is_running("sub_a"));
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active")
    );
    sup.stop_all().await;
}

// S5c-X PR6b-3: TERMINAL receipt corruption — a valid v3 bookmark whose
// `outstanding_receipt` pointer references a MISSING receipt. The receipt
// pipeline fail-stops (`RepoRequiresReopen` family), the supervisor marks
// the subscription terminal, and a resume re-trips immediately: a dangling
// pointer can never make progress. (Behavioral resume refusal — the record
// itself decodes fine, so this is not the typed `FollowerStateRefused`.)
#[tokio::test]
async fn terminal_receipt_corruption_stops_supervisor_and_refuses_resume() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let info = follower
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_info_store()
        .await
        .unwrap();
    save_bookmark_v3(
        &info,
        &BookmarkV3 {
            fence: None,
            bookmark: 0,
            clean_through: None,
            outstanding_receipt: Some([0u8; 16]),
        },
    )
    .await
    .unwrap();

    let events = single_insert_event();
    let source = Arc::new(PartialSource {
        events: rmp_serde::to_vec_named(&events).unwrap(),
        pulls: AtomicUsize::new(0),
    });
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(20)
        .with_max_iterations(2);
    sup.reconcile().await;
    wait_for_sub_state(&follower, "reopen_required").await;
    assert_eq!(
        bookmark(&follower).await,
        0,
        "no bookmark progress past the corrupt pointer"
    );
    assert_eq!(
        items_row_counter(&follower).await,
        0,
        "no events applied past the corrupt pointer"
    );

    // Prune the dead loop's registry entry, then resume administratively;
    // the next reconcile restarts the loop, which re-trips the SAME
    // corruption immediately — terminal again, still zero progress.
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));
    alter(
        &follower,
        alter_subscription("sub_a").resume().build().unwrap(),
    )
    .await;
    sup.reconcile().await;
    wait_for_sub_state(&follower, "reopen_required").await;
    assert_eq!(bookmark(&follower).await, 0);
    assert_eq!(items_row_counter(&follower).await, 0);
    assert!(
        source.pulls.load(Ordering::Acquire) >= 2,
        "the resumed loop pulled again and re-tripped at the pipeline"
    );
    sup.stop_all().await;
}

struct PartialSource {
    events: Vec<u8>,
    pulls: AtomicUsize,
}

#[async_trait]
impl ReplSource for PartialSource {
    async fn hello(&self, _node: &str) -> Result<ReplResponse, ReplError> {
        // Exercise first-Pull adoption: no successful Hello in this fixture.
        Err(ReplError::Transport("fixture starts with Pull".into()))
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from: u64,
        _limit: u32,
        _wait: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        self.pulls.fetch_add(1, Ordering::AcqRel);
        Ok(ReplResponse::Pull {
            schema_digest: None,
            repo_lineage_id: Some([1; 16]),
            history_epoch: Some(0),
            gap_first: None,
            gap_through: None,
            leader_epoch: 1,
            events: self.events.clone(),
            gap_at: None,
            current_version: 1,
        })
    }
}

type HistoryStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

// Persisted degradation is diagnostic, not a permanent Resume latch.
#[tokio::test]
async fn healthy_destination_can_resume_persisted_reopen_state() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    follower
        .mark_subscription_terminal("sub_a", "reopen_required", "previous instance")
        .await
        .unwrap();
    alter(
        &follower,
        alter_subscription("sub_a").resume().build().unwrap(),
    )
    .await;
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active")
    );
}

async fn sub_state(follower: &ShamirDb, name: &str) -> Option<String> {
    let table = follower.system_store().subscriptions_table().await.ok()?;
    let interner = table.interner_for_test().get().await.ok()?;
    let refs = shamir_db::types::common::new_map();
    let ctx = shamir_db::query::filter::FilterContext::new(interner, &refs);
    let query = shamir_db::query::read::ReadQuery::new("subscriptions").filter(
        shamir_db::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: shamir_db::query::filter::FilterValue::String(name.to_string()),
        },
    );
    let result = table.read(&query, &ctx).await.ok()?;
    result.records.first().and_then(|r| {
        r.as_value()
            .get("state")
            .and_then(|v| v.as_str())
            .map(String::from)
    })
}

/// S5c-X 2e: once a subscription is quarantined `resync_required`, the
/// destination repo it covers is excluded from DOWNSTREAM serving —
/// `ShamirDb::has_resync_required_subscription_covering` is true for the
/// covered (db, repo) and false for an uncovered one. A node with no
/// subscriptions at all must NOT be refused (regression guard for the
/// leader serve path).
#[tokio::test]
async fn resync_required_destination_is_excluded_from_downstream_serving() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await; // profile `prof_a`, Pull over `app/main`

    // Nothing quarantined yet → nothing covered (also covers the
    // "no resync_required subscriptions → false" leader-path regression).
    assert!(!follower
        .has_resync_required_subscription_covering("app", "main")
        .await
        .unwrap());

    // Quarantine exactly like the supervisor's terminal arm does.
    follower
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .unwrap();

    assert!(
        follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the covered repo must be excluded from serving"
    );
    assert!(
        !follower
            .has_resync_required_subscription_covering("app", "other")
            .await
            .unwrap(),
        "an uncovered repo in the same db must stay servable"
    );
    assert!(
        !follower
            .has_resync_required_subscription_covering("otherdb", "main")
            .await
            .unwrap(),
        "an uncovered db must stay servable"
    );
}
