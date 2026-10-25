//! S5c consumer contracts over raw fjall + runtime teardown, not simulated gaps.
//! A separate durable marker fixture retains consumer/supervisor gap refusal.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use shamir_db::engine::meta::repl::bookmark_v3::{save_bookmark_v3, BookmarkV3};
use shamir_db::engine::meta::{MetaEnvelope, MetaKey};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig, RepoInstance};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::tx::apply_replicated::apply_replicated_batch;
use shamir_db::engine::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};
use shamir_db::storage::error::{DbError, FollowerRefusalReason};
use shamir_db::storage::storage_fjall::FjallRepo;
use shamir_db::storage::types::{RecordKey, Repo};
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::alter_subscription;
use shamir_query_builder::ddl::{
    drop_subscription, list_subscriptions, repl_scope, replication_profile, replication_status,
    subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::BatchError;
use shamir_query_types::wire::repl::ReplResponse;
use shamir_tx::changefeed::{encode_gap_marker, gap_marker_key};
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

use super::supervisor_tests::{
    alter, bookmark, build_db, create_profile_and_subscription, items_row_counter,
    single_insert_event, sub_binding_generation, sub_state, wait_for_sub_state, PartialSource,
};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

// The catalogue is fresh on each boot; the DATA repo is the same raw fjall
// directory. This isolates certification from metadata/file-lock retry timing.
async fn open(path: Option<&Path>) -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await.expect("create_db");
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
            .unwrap()
            .expect("journal present");
        assert_eq!(journal.gap_at, None);
        assert_eq!(journal.events.len(), 1);
        assert_eq!(journal.events[0].commit_version, h);
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
        // max_iterations=1 completes and joins the task; the catalog's active
        // state is persisted subscription configuration, not a live-task count.
        assert_admin_state(&supervised, "active").await;
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
        let backing = FjallRepo::new(&path).expect("open stopped raw repository");
        let store = backing.store_get("__changelog__").await.unwrap();
        store
            .set(
                RecordKey::from_slice(&gap_marker_key()),
                encode_gap_marker((1, h)),
            )
            .await
            .unwrap();
        store.flush().await.unwrap();
        drop(store);
        drop(backing);
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
        // S5c-X PR8b: reads of a quarantined destination fail closed. `sub`
        // covers `app/main` and the supervisor just marked it
        // `resync_required`, so an ordinary batch read of the same repo is
        // refused with the typed `BatchError::ResyncRequired` (code
        // `resync_required`) — it no longer returns (empty) rows. All other
        // assertions above (consumer stopped, supervisor state, journal
        // semantics) are unchanged.
        let read_error = {
            let mut batch = Batch::new();
            batch.query("rows", Query::from("items"));
            follower
                .execute("app", &batch.build())
                .await
                .expect_err("quarantined read must be refused")
        };
        assert!(matches!(read_error, BatchError::ResyncRequired { .. }));
        assert_eq!(read_error.code(), Some("resync_required"));
        supervisor.stop_all().await;
        // Apply the actual payload delivered alongside the explicit GAP via
        // the receipt-only batch entry, then persist its leader bookmark as a
        // v3 record to model a follower caught up through H.
        let seeded = open(None).await;
        let outcome = apply_replicated_batch(&repo(&seeded), &events, 0, ApplyPolicy::default())
            .await
            .unwrap();
        assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
        let info = repo(&seeded).tx_info_store().await.unwrap();
        save_bookmark_v3(
            &info,
            &BookmarkV3 {
                fence: None,
                bookmark: h,
                clean_through: None,
                outstanding_receipt: None,
            },
        )
        .await
        .unwrap();
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

// ---------------------------------------------------------------------------
// S5c-X PR6b-3 round 3 — supervisor terminal-state tests (moved here from the
// dissolved `supervisor_receipt_terminal_tests` module for the layout ratchet).
// ---------------------------------------------------------------------------

// Unit tests for the gate-init refusal mapping itself (`refuse_gate_init`):
// the supervisor cannot reach a FAILING `tx_gate()` through the integration
// fixture below — the gate OnceCell is initialized during open recovery
// (engine tx/recovery step 1) and open itself refuses legacy/malformed bytes,
// so the supervisor always sees the cached, successfully-initialized gate.
// These drive every error kind DIRECTLY against the real supervisor + real
// terminal persistence.
#[tokio::test]
async fn gate_init_error_legacy_refusal_is_typed_and_marks_terminal() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let source = Arc::new(PartialSource {
        events: Vec::new(),
        pulls: AtomicUsize::new(0),
    });
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "unit")
        .with_poll_wait_ms(20)
        .with_max_iterations(2);
    // Counterfactual: flattening the helper's match to a blanket
    // `ReplError::Start(error.to_string())` (the round-2 shape) turns this
    // red — the return would be `Start` and `resync_required` never persists.
    let error = sup
        .refuse_gate_init(
            "sub_a",
            sub_binding_generation(&follower, "sub_a")
                .await
                .expect("row carries binding_generation"),
            DbError::FollowerStateRefused {
                reason: FollowerRefusalReason::LegacyBookmarkFormat,
            },
        )
        .await;
    assert!(
        matches!(
            error,
            ReplError::FollowerStateRefused {
                reason: FollowerRefusalReason::LegacyBookmarkFormat
            }
        ),
        "typed refusal must stay machine-readable, got {error:?}"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "typed refusal must persist terminal resync_required"
    );
    sup.stop_all().await;
}

#[tokio::test]
async fn gate_init_error_malformed_refusal_is_typed_and_marks_terminal() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let source = Arc::new(PartialSource {
        events: Vec::new(),
        pulls: AtomicUsize::new(0),
    });
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "unit")
        .with_poll_wait_ms(20)
        .with_max_iterations(2);
    // Counterfactual: same as the legacy variant — a blanket `Start` mapping
    // would return `Start` and never persist `resync_required`.
    let error = sup
        .refuse_gate_init(
            "sub_a",
            sub_binding_generation(&follower, "sub_a")
                .await
                .expect("row carries binding_generation"),
            DbError::FollowerStateRefused {
                reason: FollowerRefusalReason::MalformedBookmark,
            },
        )
        .await;
    assert!(
        matches!(
            error,
            ReplError::FollowerStateRefused {
                reason: FollowerRefusalReason::MalformedBookmark
            }
        ),
        "typed refusal must stay machine-readable, got {error:?}"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "typed refusal must persist terminal resync_required"
    );
    sup.stop_all().await;
}

#[tokio::test]
async fn gate_init_error_other_error_keeps_start_and_marks_nothing() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let before = sub_state(&follower, "sub_a").await;
    let source = Arc::new(PartialSource {
        events: Vec::new(),
        pulls: AtomicUsize::new(0),
    });
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "unit")
        .with_poll_wait_ms(20)
        .with_max_iterations(2);
    let error = sup
        .refuse_gate_init(
            "sub_a",
            sub_binding_generation(&follower, "sub_a")
                .await
                .expect("row carries binding_generation"),
            DbError::Storage("gate init i/o".into()),
        )
        .await;
    assert!(
        matches!(error, ReplError::Start(_)),
        "non-refusal gate error must keep the generic retryable Start mapping, got {error:?}"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await,
        before,
        "non-refusal gate error must mark nothing"
    );
    sup.stop_all().await;
}

// S5c-X PR6b-3 round 3 (re-commented round 4): this seeds legacy bytes into
// the ALREADY-OPEN repo, so it exercises the admission-time classifier arm
// (`classify_follower_bookmark` in the supervisor's resume path) and terminal
// persistence — NOT the early gate-init arm, which the unit tests above cover
// directly: the gate OnceCell initializes during open recovery and open
// itself refuses legacy bytes, so a failing `tx_gate()` is unreachable here.
#[tokio::test]
async fn legacy_bookmark_gate_refusal_marks_typed_terminal() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    // Seed legacy bookmark bytes BEFORE any reconcile: gate init classifies
    // the destination and refuses (the tests own the legacy fixture bytes —
    // no production legacy writer exists).
    let info = follower
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_info_store()
        .await
        .unwrap();
    info.set(
        RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes()),
        Bytes::from(MetaEnvelope::new(10u64).encode().unwrap()),
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
    // Synchronous in start_subscription: typed arm marks and returns before
    // any loop spawns.
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "typed refusal must persist terminal resync_required"
    );
    assert!(!sup.is_running("sub_a"));
    assert_eq!(source.pulls.load(Ordering::Acquire), 0);

    // The subscription is no longer active → reconcile must not start it.
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));
    assert_eq!(source.pulls.load(Ordering::Acquire), 0);
    sup.stop_all().await;
}

/// A [`PartialSource`] that, on its FIRST pull, seeds a durable v3 bookmark
/// with a DANGLING outstanding-receipt pointer into the destination info
/// store, then serves events as usual — modelling "initial pull, then the
/// receipt pipeline fail-stops".
struct SeedingSource {
    events: Vec<u8>,
    pulls: AtomicUsize,
    follower: Arc<ShamirDb>,
    seeded: AtomicBool,
}

#[async_trait]
impl ReplSource for SeedingSource {
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
        if !self.seeded.swap(true, Ordering::AcqRel) {
            let info = self
                .follower
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
                    outstanding_receipt: Some([9u8; 16]),
                },
            )
            .await
            .unwrap();
        }
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

// S5c-X PR6b-3 round 3: the INITIAL pipeline trip still marks terminal —
// the loop classifies the freshly seeded v3 as Native, resume lookup hits
// the dangling receipt pointer (`RepoRequiresReopen`), and the supervisor's
// ReopenRequired arm persists `reopen_required`; every later start attempt
// is refused at admission with zero further pulls.
#[tokio::test]
async fn initial_pipeline_trip_marks_terminal_then_resume_refused_at_admission() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let events = single_insert_event();
    let source = Arc::new(SeedingSource {
        events: rmp_serde::to_vec_named(&events).unwrap(),
        pulls: AtomicUsize::new(0),
        follower: follower.clone(),
        seeded: AtomicBool::new(false),
    });
    let shared = source.clone();
    let factory: ReplSourceFactory = Arc::new(move |_| shared.clone());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(20)
        .with_max_iterations(2);
    sup.reconcile().await;

    // No v3 yet → classify Fresh → admission passes → loop spawns → pull #1
    // seeds the dangling pointer and serves events → admit_follower classifies
    // Native → resume lookup fails with `RepoRequiresReopen` → the loop maps
    // it to `ReopenRequired` → the supervisor marks terminal.
    wait_for_sub_state(&follower, "reopen_required").await;
    assert_eq!(
        source.pulls.load(Ordering::Acquire),
        1,
        "exactly the initial pull must happen"
    );
    assert_eq!(bookmark(&follower).await, 0, "no events applied");
    assert_eq!(items_row_counter(&follower).await, 0);

    // Prune the dead loop; admission now sees Native + dangling pointer.
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));

    // An administrative resume stays refused at admission.
    alter(
        &follower,
        alter_subscription("sub_a").resume().build().unwrap(),
    )
    .await;
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));
    assert_eq!(
        source.pulls.load(Ordering::Acquire),
        1,
        "the refused resume must not perform any pull"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("reopen_required")
    );
    sup.stop_all().await;
}

// S5c-X PR6b-3 round 3: a valid v3 bookmark whose `outstanding_receipt`
// pointer references UNDECODABLE receipt bytes must be terminal exactly
// like a MISSING one: `receipt_pointer_resolves` treats corrupt bytes as
// unresolved, so the supervisor's admission pre-check marks
// `reopen_required` BEFORE any loop is spawned — zero pulls, zero progress,
// and an administrative resume stays refused.
#[tokio::test]
async fn malformed_receipt_bytes_stop_supervisor_and_refuse_resume() {
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
            outstanding_receipt: Some([1u8; 16]),
        },
    )
    .await
    .unwrap();
    // The receipt key itself is crate-private to the engine; rebuild it here.
    info.set(
        RecordKey::from_slice(
            shamir_db::engine::meta::MetaKey::ReplicationReceipt
                .as_record_id()
                .as_bytes(),
        ),
        Bytes::from_static(b"corrupted receipt bytes"),
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
    // Admission pre-check: classify Native → pointer resolves → get finds
    // bytes, decode fails → unresolved → terminal reopen_required, NO loop.
    wait_for_sub_state(&follower, "reopen_required").await;
    assert!(!sup.is_running("sub_a"));
    assert_eq!(
        source.pulls.load(Ordering::Acquire),
        0,
        "admission refuses before any pull"
    );
    assert_eq!(bookmark(&follower).await, 0);
    assert_eq!(items_row_counter(&follower).await, 0);

    // An administrative resume stays refused at admission.
    alter(
        &follower,
        alter_subscription("sub_a").resume().build().unwrap(),
    )
    .await;
    sup.reconcile().await;
    assert!(!sup.is_running("sub_a"));
    assert_eq!(
        source.pulls.load(Ordering::Acquire),
        0,
        "the refused resume must not perform any pull"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("reopen_required")
    );
    sup.stop_all().await;
}

/// S5c-X PR8b-2 evidence row 5 (stale terminal callback): a callback carrying
/// a binding generation that no longer matches the row's durable binding is a
/// logged no-op — the row's `state` does NOT change and the quarantine cache
/// does not grow. The CORRECT generation flips the row. In-process, no
/// timing races: both calls go through the same
/// `mark_subscription_resync_required_generation` seam the supervisor uses.
/// The row's REAL binding generation is read from the catalogue; the stale
/// call uses a far-future generation.
#[tokio::test]
async fn stale_generation_callback_does_not_mark_replacement() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Nothing quarantined yet.
    assert!(!follower
        .has_resync_required_subscription_covering("app", "main")
        .await
        .unwrap());

    // The row's REAL durable generation (durable-sequence values are
    // monotonic but not small predictable integers).
    let gen = sub_binding_generation(&follower, "sub_a")
        .await
        .expect("row exists");
    assert!(gen >= 1);

    // STALE generation (far past the row's): a no-op — no state flip, no
    // cache growth.
    follower
        .mark_subscription_resync_required_generation("sub_a", 100, 42, gen + 100)
        .await
        .unwrap();
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active"),
        "a stale-generation callback must not mark the row"
    );
    assert!(
        !follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "a stale-generation callback must not grow the quarantine cache"
    );

    // The CORRECT generation (the row's durable one) marks the row and
    // covers exactly its bound destination.
    follower
        .mark_subscription_resync_required_generation("sub_a", 100, 42, gen)
        .await
        .unwrap();
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "the current-generation callback must mark the row"
    );
    assert!(
        follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the bound destination must be excluded from serving"
    );
    assert!(
        !follower
            .has_resync_required_subscription_covering("app", "other")
            .await
            .unwrap(),
        "an uncovered repo must stay servable"
    );
}

/// S5c-X PR8b-2 D1: a DROP + identical re-CREATE of the same subscription
/// name must be stamped with a STRICTLY GREATER durable binding generation
/// (the monotonic `next_generation` floor over the system repo's durable
/// MVCC version), so the OLD task's terminal callback
/// (`mark_subscription_resync_required_generation` with the pre-drop
/// generation) is a logged no-op on the replacement row, while the CURRENT
/// task's callback (with the recreated row's generation) marks it
/// `resync_required` and quarantines exactly its bound destination.
#[tokio::test]
async fn replacement_generation_blocks_old_task_callback() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // 1. Read the raw row through the public catalogue read path (mirrors
    //    `supervisor.rs::read_all`) for the pre-drop binding generation.
    let gen_before = sub_binding_generation(&follower, "sub_a")
        .await
        .expect("row exists after CREATE");

    // 2. DROP while the subscription is active — must succeed.
    let mut b = Batch::new();
    b.id(1);
    b.op("drop", drop_subscription("sub_a"));
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("drop active subscription");
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        None,
        "the dropped row must be gone"
    );

    // 3. Re-create identically → the generation strictly increases.
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_a"),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("re-create subscription");
    let gen_after = sub_binding_generation(&follower, "sub_a")
        .await
        .expect("row exists after re-CREATE");
    assert!(
        gen_after > gen_before,
        "generation must strictly increase across DROP + re-CREATE: \
         {gen_before} -> {gen_after}"
    );

    // 4. OLD task's callback (pre-drop generation) is a no-op.
    follower
        .mark_subscription_resync_required_generation("sub_a", 100, 42, gen_before)
        .await
        .unwrap();
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active"),
        "the old task's generation must not mark the replacement row"
    );
    assert!(
        !follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the old task's generation must not grow the quarantine cache"
    );

    // 5. CURRENT task's callback (the recreated row's generation) marks it.
    follower
        .mark_subscription_resync_required_generation("sub_a", 100, 42, gen_after)
        .await
        .unwrap();
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "the current generation must mark the row"
    );
    assert!(
        follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the bound destination must be excluded from serving"
    );
    assert!(
        !follower
            .has_resync_required_subscription_covering("app", "other")
            .await
            .unwrap(),
        "an uncovered repo must stay servable"
    );
}
