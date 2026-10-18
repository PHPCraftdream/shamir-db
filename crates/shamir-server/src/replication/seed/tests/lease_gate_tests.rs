//! S5c-X round 5 — supervisor lease/barrier tests (W3).
//!
//! Fixtures replicate `supervisor_tests.rs` locally (leader/follower
//! `build_db`, builder-batch catalogue writes, a supervisor factory) —
//! nothing is imported from that module. All synchronisation is Notify-,
//! join- and gate-based: `wait_for_subscription` joins the loop tasks,
//! `await_journal_progress` notify-parks on engine completion, real
//! catalogue seams (`SeedCatalogueFlushFaultForTest`, the lease
//! `apply_gate`, the final-write seam) block real mutators, and
//! `tokio::time::pause` advances the B3 retry sleeps. NO sleeps and NO
//! yield budgets anywhere in this file.
//!
//! Scenarios:
//!   * T1 — create → reconcile starts a FRESH subscription: the follower
//!     bookmark reaches the leader's version.
//!   * T2 — a REAL catalogue-flush fault at the start barrier defers the
//!     start (nothing spawned, nothing marked, lease invalidated); the
//!     retrying reconcile starts it.
//!   * T3 — a REAL flush fault fails the PAUSE of a LIVE follower: the row
//!     reads `paused`, the lease is invalid, the loop is stopped by the
//!     system, and reconcile never restarts a paused row.
//!   * T4 — an obsolete G1 loop parked mid-Pull applies nothing after the
//!     binding was replaced by a strictly-greater G2 (incl. REAL DROP quiesce).
//!   * T5 — the start-race hook replaces the row between lease-take and
//!     spawn: the start is dropped, nothing spawns, nothing is marked.
//!   * A1 — pause/resume back-to-back restarts and applies new events; a
//!     CANCELLED quiesce (dropped pause future) leaves an active row
//!     recoverable via the stale-handle sweep.
//!   * A2 — a second supervisor defers while the first follower's lease is
//!     valid and takes over only after invalidation.
//!   * T7r — a CREATE dropped at the final-write boundary leaves a
//!     `pending_durable` row that reconcile never starts.
//!   * B3 — terminal marking retries through REAL flush faults until the row
//!     ends `resync_required`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::shamir_db::shamir_db::{FinalWriteBarrierForTest, SeedCatalogueFlushFaultForTest};
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{drop_subscription, repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::{BatchError, BatchOp, BatchResponse};
use shamir_query_types::wire::repl::ReplResponse;
use futures_util::poll; // poll! re-export
use tokio::sync::Notify;

/// A boxed follower-catalogue batch future (`poll!`-able / droppable mid-flight).
type BoxedBatch = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<BatchResponse, BatchError>> + Send>,
>;

use crate::replication::error::ReplError;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// In-memory `ShamirDb` with db `app`, repos `main` + `other` (each a
/// `users`-less `items` table), owned by `alice`. Used for the leader
/// (rows land in `app/main`) and the follower (bindings may target either).
async fn build_db() -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username("alice"));
    shamir.create_db_as("app", owner.clone()).await;
    for repo in ["main", "other"] {
        let cfg =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        shamir
            .add_repo_as("app", cfg, owner.clone())
            .await
            .expect("add repo");
    }
    Arc::new(shamir)
}

/// Write `n` rows into `app/main/items` on the leader (fixture write
/// helper). Callers capture `leader_version` and pass it to
/// [`await_journal_ready`] — journal durability is awaited, never assumed.
async fn write_rows(leader: &ShamirDb, base: usize, n: usize) {
    let owner = Actor::User(principal64_from_username("alice"));
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
}

/// Leader commit version of `app/main` after the fixture writes.
async fn leader_version(leader: &ShamirDb) -> u64 {
    leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version")
}

/// Notify-based journal readiness: parks until the repo's completion prefix
/// covers `version` — no sleeps, no polling (round 5 replaces the
/// "long-poll covers durability" assumption).
async fn await_journal_ready(db: &ShamirDb, repo: &str, version: u64) {
    db.get_db("app")
        .and_then(|d| d.get_repo(repo))
        .expect("repo exists")
        .await_journal_progress(version)
        .await
        .expect("journal readiness");
}

/// Follower `app/{repo}` replication bookmark.
async fn bookmark(follower: &ShamirDb, repo: &str) -> u64 {
    follower
        .get_db("app")
        .and_then(|d| d.get_repo(repo))
        .expect("follower repo")
        .replication_bookmark()
        .await
        .expect("bookmark")
}

/// Create profile `prof_a` over `app/main` + ACTIVE subscription `sub_a`
/// bound to it — both on the FOLLOWER catalogue (builder batches only).
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

/// Create profile `prof_b` over `app/other` on the follower.
async fn create_profile_b(follower: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cp",
        replication_profile("prof_b").stream(
            repl_scope("app").repo("other").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("create profile prof_b");
}

/// Re-create ACTIVE `sub_a` over `prof_b` (the G2 replacement in T4/T5).
async fn recreate_sub_a_over_prof_b(follower: &ShamirDb, id: u32) {
    let mut b = Batch::new();
    b.id(id);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_b"),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("re-create sub_a over prof_b");
}

/// Run one builder op against the follower catalogue.
async fn run_op(follower: &ShamirDb, op: BatchOp) {
    let mut b = Batch::new();
    b.id(1);
    b.op("op", op);
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("catalogue batch");
}

/// The same batch as a BOXED future, for seam tests that must `poll!` it or
/// drop it mid-flight (the real PAUSE/DROP quiesce barriers).
fn op_future(follower: Arc<ShamirDb>, op: BatchOp) -> BoxedBatch {
    let mut b = Batch::new();
    b.id(1);
    b.op("op", op);
    let request = b.to_request_via_msgpack();
    Box::pin(async move { follower.execute("app", &request).await })
}

/// `ALTER SUBSCRIPTION sub_a PAUSE` as a boxed future (T3/A1).
fn pause_op_future(follower: Arc<ShamirDb>) -> BoxedBatch {
    op_future(
        follower,
        shamir_query_builder::ddl::alter_subscription("sub_a")
            .pause()
            .build()
            .unwrap(),
    )
}

/// Supervisor factory over an `InProcessReplSource` on the leader, with a
/// generous long-poll budget (deterministic journal catch-up, no sleeps).
fn supervisor(follower: Arc<ShamirDb>, leader: Arc<ShamirDb>) -> SubscriptionSupervisor {
    let factory: ReplSourceFactory = Arc::new(move |_sub: &Subscription| {
        Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
    });
    SubscriptionSupervisor::new(follower, factory, "follower-1").with_poll_wait_ms(2000)
}

/// Raw catalogue read of one subscription row (mirrors
/// `supervisor.rs::read_all`: the FilterContext is built engine-side).
async fn sub_row(follower: &ShamirDb, name: &str) -> Option<QueryValue> {
    let table = follower.system_store().subscriptions_table().await.ok()?;
    let refs = shamir_db::types::common::new_map();
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .ok()?;
    let query = shamir_db::query::read::ReadQuery::new("subscriptions");
    let records = table.read(&query, &ctx).await.ok()?.records;
    records
        .iter()
        .find(|r| {
            r.as_value()
                .get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|n| n == name)
        })
        .map(|r| r.as_value().into_owned())
}

async fn sub_state(follower: &ShamirDb, name: &str) -> Option<String> {
    sub_row(follower, name)
        .await
        .and_then(|r| r.get("state").and_then(|v| v.as_str()).map(String::from))
}

async fn sub_generation(follower: &ShamirDb, name: &str) -> u64 {
    sub_row(follower, name)
        .await
        .and_then(|r| r.get("binding_generation").and_then(|v| v.as_u64()))
        .expect("row carries a binding generation")
}

// ---------------------------------------------------------------------------
// T1 — create → reconcile starts a fresh subscription
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_then_reconcile_starts_fresh_subscription() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let version = leader_version(&leader).await;
    await_journal_ready(&leader, "main", version).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // No faults — the fresh subscription MUST start on the first reconcile.
    let sup = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await; // joins the loop

    assert!(
        bookmark(&follower, "main").await >= version,
        "the fresh subscription must have started and caught up"
    );
}

// ---------------------------------------------------------------------------
// T2 — a REAL start-barrier flush failure defers the start; retry starts it
// ---------------------------------------------------------------------------

/// COUNTERFACTUAL: reverting the supervisor's barrier error handling (the
/// flush failure no longer defers/invalidates) makes one half red — either
/// the follower starts UNDER the armed fault (first-half `is_running`/
/// `active_count`/lease assertions) or the retry is refused by the stale
/// valid lease the failed barrier left behind (second-half bookmark).
#[tokio::test]
async fn start_barrier_failure_defers_start_until_retry() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);

    // Inject at the REAL catalogue-flush seam — AFTER the creation batches,
    // so the barrier flush of the START is the first faulted flush.
    let fault = SeedCatalogueFlushFaultForTest::arm(&follower);

    sup.reconcile().await;
    assert!(
        !sup.is_running("sub_a"),
        "the failed start barrier must not spawn anything"
    );
    assert_eq!(sup.active_count(), 0);
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active"),
        "nothing may be marked on a deferred start"
    );
    assert_eq!(bookmark(&follower, "main").await, 0);
    assert!(
        follower.subscription_lease("sub_a").is_none(),
        "the failed barrier must invalidate (remove) its just-taken lease"
    );

    // Clear the fault: the retrying reconcile runs the barrier for real and
    // starts the loop.
    drop(fault);
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await;
    assert!(
        bookmark(&follower, "main").await >= version,
        "the retried reconcile must start the subscription"
    );
}

// ---------------------------------------------------------------------------
// T3 — a REAL flush fault fails the PAUSE of a LIVE follower
// ---------------------------------------------------------------------------

/// COUNTERFACTUALS: without the pause's quiesce-before-mutation the lease
/// would stay valid and the loop would keep long-polling
/// (`wait_for_subscription` never sees the exit → hang/red); without W2's
/// exit-path lease invalidation the later restart could not be taken
/// (covered by the T1/A1 tests' lease assertions).
#[tokio::test]
async fn pause_flush_failure_stops_live_follower_and_never_restarts() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;
    await_journal_ready(&leader, "main", version).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Run-forever supervisor: the loop stays LIVE, long-polling.
    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;
    assert!(sup.is_running("sub_a"), "the loop must be live");

    // Arm the REAL seam, then execute the PAUSE batch: quiesce → row flip →
    // flush FAILS → the batch errors, but the row is already `paused` and
    // the lease already invalid.
    let fault = SeedCatalogueFlushFaultForTest::arm(&follower);
    let pause = pause_op_future(follower.clone());
    assert!(
        pause.await.is_err(),
        "the armed flush fault must fail the PAUSE batch"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await,
        Some("paused".to_string()),
        "the LIVE row must read paused (raw sub_state)"
    );
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        Some(false),
        "the pause's quiesce must have invalidated the lease"
    );

    // The loop is stopped BY THE SYSTEM: joins when the registry entry is
    // still there; a no-op when W2's self-deregistration already removed it
    // — either way it returns only after the loop exited.
    sup.wait_for_subscription("sub_a").await;
    assert!(!sup.is_running("sub_a"));

    drop(fault);
    sup.reconcile().await; // a paused row is never restarted
    assert_eq!(sup.active_count(), 0);
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("paused")
    );
}

// ---------------------------------------------------------------------------
// A1 — pause/resume back-to-back; cancelled quiesce recovery
// ---------------------------------------------------------------------------

/// COUNTERFACTUAL: reverting W2's stale-handle sweep (or the exit-path lease
/// invalidation) leaves the dead handle registered / its lease valid → the
/// restarted loop never spawns → the bookmark is stuck at v1 → red.
#[tokio::test]
async fn pause_resume_back_to_back_restarts_and_applies_new_event() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let v1 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v1).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await; // loop#1 applied, exited,
    assert!(bookmark(&follower, "main").await >= v1); // self-deregistered
    let gen = sub_generation(&follower, "sub_a").await;

    // PAUSE then RESUME back-to-back: the row ends active with the SAME
    // generation.
    pause_op_future(follower.clone()).await.expect("pause");
    op_future(
        follower.clone(),
        shamir_query_builder::ddl::alter_subscription("sub_a")
            .resume()
            .build()
            .unwrap(),
    )
    .await
    .expect("resume");
    assert_eq!(sub_state(&follower, "sub_a").await.as_deref(), Some("active"));
    assert_eq!(
        sub_generation(&follower, "sub_a").await,
        gen,
        "pause/resume must not bump the binding generation"
    );

    // NEW leader rows BEFORE the restarting reconcile; journal readiness is
    // awaited, never assumed.
    write_rows(&leader, 10, 2).await;
    let v2 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v2).await;

    sup.notify_changed().await; // sweeps the stale handle, RESTARTS
    sup.wait_for_subscription("sub_a").await; // joins loop#2
    assert!(
        bookmark(&follower, "main").await >= v2,
        "the restarted loop must have applied the NEW events"
    );
}

/// A CANCELLED quiesce — the PAUSE future dropped while its write gate is
/// blocked — leaves the row active and the lease invalid; the stale-handle
/// sweep then restarts the follower under a FRESH lease.
///
/// COUNTERFACTUAL: reverting the stale sweep → no restart → the bookmark is
/// stuck at v1 → red.
#[tokio::test]
async fn cancelled_quiesce_leaves_active_row_recoverable() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let v1 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v1).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Run-forever: loop#1 stays LIVE (its lease stays valid and present)
    // while it long-polls between iterations.
    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;
    // Deterministic: loop#1 applied v1 (notify-based follower-side
    // readiness) and is now parked in its long-poll holding NO guard.
    await_journal_ready(&follower, "main", v1).await;

    // Block the quiesce write gate: hold the live lease's read guard.
    let lease = follower
        .subscription_lease("sub_a")
        .expect("loop#1 is live, its lease must exist");
    let gen = lease.generation;
    let g = lease.apply_gate.read().await;

    // The REAL PAUSE batch: its quiesce stores valid=false, then parks on
    // the write gate → Pending.
    let mut pause = pause_op_future(follower.clone());
    assert!(
        matches!(poll!(pause.as_mut()), std::task::Poll::Pending),
        "the pause quiesce must be blocked by the held read guard"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active"),
        "the cancelled quiesce must leave the row ACTIVE"
    );
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(Ordering::Acquire)),
        Some(false),
        "the quiesce already flipped the lease's valid flag"
    );

    // CANCELLED: drop the parked PAUSE future (never abort), then the guard.
    drop(pause);
    drop(g);

    // RESUME succeeds: row active, SAME generation.
    op_future(
        follower.clone(),
        shamir_query_builder::ddl::alter_subscription("sub_a")
            .resume()
            .build()
            .unwrap(),
    )
    .await
    .expect("resume");
    assert_eq!(sub_state(&follower, "sub_a").await.as_deref(), Some("active"));
    assert_eq!(
        sub_generation(&follower, "sub_a").await,
        gen,
        "resume must not bump the binding generation"
    );

    // NEW leader rows + readiness BEFORE the restarting reconcile.
    write_rows(&leader, 10, 2).await;
    let v2 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v2).await;

    sup.notify_changed().await; // sweeps the stale handle, restarts
    sup.wait_for_subscription("sub_a").await; // joins the restarted loop
    assert!(
        bookmark(&follower, "main").await >= v2,
        "the restarted loop must apply the NEW events under its FRESH lease"
    );
}

// ---------------------------------------------------------------------------
// A2 — a second supervisor defers, then takes over after invalidation
// ---------------------------------------------------------------------------

/// COUNTERFACTUAL: reverting W1's refusal (takeover-on-take) makes sup_b
/// kill sup_a's live follower mid-flight at its very first reconcile —
/// sup_a.is_running("sub_a") goes false during the refusal phase → red.
#[tokio::test]
async fn second_supervisor_defers_then_takes_over_after_invalidation() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let v1 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v1).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // One shared factory so both supervisors build identical sources.
    let factory: ReplSourceFactory = {
        let leader = leader.clone();
        Arc::new(move |_sub: &Subscription| {
            Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
        })
    };
    let sup_a = SubscriptionSupervisor::new(follower.clone(), factory.clone(), "follower-a")
        .with_poll_wait_ms(2000);
    let sup_b = SubscriptionSupervisor::new(follower.clone(), factory, "follower-b")
        .with_max_iterations(1);

    // sup_a runs the LIVE follower L1.
    sup_a.reconcile().await;
    assert!(sup_a.is_running("sub_a"), "L1 must be live");

    // sup_b's reconcile is REFUSED: L1's lease is valid, so there is already
    // exactly one live follower — sup_b defers (starts nothing) and sup_a
    // stays running.
    sup_b.reconcile().await;
    assert_eq!(sup_b.active_count(), 0, "the refusal must start nothing");
    assert!(
        sup_a.is_running("sub_a"),
        "the refusal must never kill the live follower (W1)"
    );

    // sup_a shuts down: cancel → loops exit gracefully → W2's exit path
    // invalidates L1 and self-deregisters; stop_all joins them.
    sup_a.stop_all().await;
    assert!(!sup_a.is_running("sub_a"));

    // NEW leader rows + readiness BEFORE sup_b's takeover reconcile.
    write_rows(&leader, 10, 2).await;
    let v2 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v2).await;

    // sup_b now takes over under a FRESH lease L2 and applies the new rows.
    sup_b.reconcile().await;
    sup_b.wait_for_subscription("sub_a").await;
    assert!(
        bookmark(&follower, "main").await >= v2,
        "sup_b must take over after the invalidation and catch up"
    );
}

// ---------------------------------------------------------------------------
// T4 — an obsolete loop parked mid-Pull applies nothing after replacement
// ---------------------------------------------------------------------------

/// Wrapper source: the FIRST `pull` parks on a Notify (before the loop can
/// reach its pre-apply guard); later pulls delegate straight through.
struct ParkedPullSource {
    inner: Arc<dyn ReplSource>,
    parked: Arc<Notify>,
    park: Arc<Notify>,
    parked_once: AtomicBool,
}

#[async_trait]
impl ReplSource for ParkedPullSource {
    async fn hello(
        &self,
        node_id: &str,
    ) -> Result<shamir_query_types::wire::repl::ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<shamir_query_types::wire::repl::ReplResponse, ReplError> {
        if !self.parked_once.swap(true, Ordering::SeqCst) {
            // Park BEFORE returning: the loop is mid-Pull, holding no guard.
            self.parked.notify_one();
            self.park.notified().await;
        }
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

#[tokio::test]
async fn obsolete_follower_parked_in_pull_applies_nothing_after_replacement() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let parked = Arc::new(Notify::new());
    let park = Arc::new(Notify::new());
    let leader_for_factory = leader.clone();
    let parked_for_factory = parked.clone();
    let park_for_factory = park.clone();
    let factory: ReplSourceFactory = Arc::new(move |_sub: &Subscription| {
        Arc::new(ParkedPullSource {
            inner: Arc::new(InProcessReplSource::new((*leader_for_factory).clone())),
            parked: parked_for_factory.clone(),
            park: park_for_factory.clone(),
            parked_once: AtomicBool::new(false),
        }) as Arc<dyn ReplSource>
    });
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_poll_wait_ms(2000)
        .with_max_iterations(1);

    sup.reconcile().await; // starts the G1 loop over app/main
    let gen1 = sub_generation(&follower, "sub_a").await;
    parked.notified().await; // the G1 Pull is now parked (no guard held)

    // While parked: take the LIVE lease and block the REAL DROP's quiesce —
    // the held read guard keeps the drop's write gate parked → Pending.
    // COUNTERFACTUAL: without the quiesce write-gate the DROP poll would be
    // Ready immediately → red.
    let lease = follower
        .subscription_lease("sub_a")
        .expect("the G1 loop is live, its lease must exist");
    let g = lease.apply_gate.read().await;
    let mut drop_future = op_future(follower.clone(), drop_subscription("sub_a"));
    assert!(
        matches!(
            poll!(drop_future.as_mut()),
            std::task::Poll::Pending
        ),
        "the REAL DROP's quiesce must be blocked by the held read guard"
    );
    // Destination still intact while the DROP is parked mid-quiesce.
    assert!(
        follower
            .read_changelog_from_journal("app", "main", 0, 100)
            .await
            .unwrap()
            .unwrap()
            .events
            .is_empty(),
        "app/main history must be intact while the DROP is parked"
    );
    drop(g); // release the guard → the REAL DROP completes
    drop_future.await.expect("DROP sub_a completes");

    // Re-create sub_a over prof_b (G2, strictly greater).
    create_profile_b(&follower).await;
    recreate_sub_a_over_prof_b(&follower, 2).await;
    let gen2 = sub_generation(&follower, "sub_a").await;
    assert!(gen2 > gen1, "G2 must be strictly greater: {gen1} -> {gen2}");

    // Release the G1 pull: the events arrive, the PRE-APPLY lease check
    // finds valid == false, and the loop exits cleanly without applying.
    park.notify_one();
    sup.wait_for_subscription("sub_a").await; // joins the (G1) loop task

    assert_eq!(
        bookmark(&follower, "main").await,
        0,
        "the obsolete G1 loop must not apply app/main events"
    );
    assert_eq!(
        bookmark(&follower, "other").await,
        0,
        "nothing may be applied to the G2 destination by the obsolete loop"
    );
    // HISTORY (not just bookmarks): the destinations are unchanged.
    assert!(
        follower
            .read_changelog_from_journal("app", "main", 0, 100)
            .await
            .unwrap()
            .unwrap()
            .events
            .is_empty(),
        "app/main history must be unchanged"
    );
    assert!(
        follower
            .read_changelog_from_journal("app", "other", 0, 100)
            .await
            .unwrap()
            .unwrap()
            .events
            .is_empty(),
        "app/other history must be unchanged"
    );
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active"),
        "the G2 row must stay live and unmarked"
    );
}

// ---------------------------------------------------------------------------
// T5 — start-race hook replacement prevents spawn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_race_hook_replacement_prevents_spawn() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // The CURRENT G1 subscription literal (gen + destinations from the row).
    let g1_sub = Subscription {
        name: "sub_a".to_string(),
        upstream: "tcp://leader:9000".to_string(),
        publication: "pub_a".to_string(),
        profile: "prof_a".to_string(),
        state: "active".to_string(),
        binding_generation: sub_generation(&follower, "sub_a").await,
        binding_destinations: vec![("app".to_string(), "main".to_string())],
    };

    // The hook (awaited between lease-take and spawn) replaces the binding:
    // DROP sub_a, create prof_b over app/other, re-create sub_a over it.
    let hook_follower = follower.clone();
    let sup = supervisor(follower.clone(), build_db().await)
        .with_max_iterations(1)
        .with_start_race_hook(Arc::new(move || {
            let follower = hook_follower.clone();
            Box::pin(async move {
                run_op(&follower, drop_subscription("sub_a")).await;
                create_profile_b(&follower).await;
                recreate_sub_a_over_prof_b(&follower, 2).await;
            })
        }));

    let started = sup.start_subscription(&g1_sub).await;
    assert!(
        matches!(started, Ok(false)),
        "the superseded start must be dropped: {started:?}"
    );
    assert!(
        !sup.is_running("sub_a"),
        "nothing may spawn on a stale lease"
    );
    assert_eq!(sup.active_count(), 0);

    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active"),
        "the G2 row must be unmarked"
    );
    assert_eq!(bookmark(&follower, "main").await, 0);
    assert_eq!(bookmark(&follower, "other").await, 0);
}

// ---------------------------------------------------------------------------
// T7r — a CREATE dropped at the final-write boundary is never started
// ---------------------------------------------------------------------------

/// COUNTERFACTUAL: if the start gate accepted a `pending_durable` row,
/// `start_subscription` would take a lease and spawn a loop → red.
#[tokio::test]
async fn dropped_creation_pending_row_is_never_started_by_reconcile() {
    let leader = build_db().await;
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Arm the final-write seam, then drive a SECOND creation (sub_b) to the
    // boundary and DROP it there.
    let seam = FinalWriteBarrierForTest::arm(&follower);
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "cs",
        subscription("sub_b", "tcp://leader:9000", "pub_a", "prof_a"),
    );
    let request = b.to_request_via_msgpack();
    let follower_for_create = follower.clone();
    let mut create = Box::pin(async move { follower_for_create.execute("app", &request).await });
    let mut wait = Box::pin(async { seam.arrived().notified().await });
    tokio::select! {
        biased;
        _ = &mut wait => {}
        _ = create.as_mut() => panic!("creation ran past the final-write seam"),
    }
    drop(create); // DROP the parked future — never abort
    seam.disarm();

    // The dropped creation left sub_b `pending_durable` and lease-less.
    assert_eq!(
        sub_state(&follower, "sub_b").await.as_deref(),
        Some("pending_durable"),
        "the final write must never have started"
    );
    assert!(follower.subscription_lease("sub_b").is_none());

    // Reconcile (no faults): the start gate must refuse the pending row —
    // nothing spawns, no lease is ever issued, bookmark stays 0.
    let sup = supervisor(follower.clone(), leader).with_max_iterations(1);
    sup.reconcile().await;
    assert!(!sup.is_running("sub_b"), "a pending row must never start");
    assert_eq!(sup.active_count(), 0);
    assert!(
        follower.subscription_lease("sub_b").is_none(),
        "no lease may ever be issued for a pending row"
    );
    assert_eq!(bookmark(&follower, "main").await, 0);
    assert_eq!(
        sub_state(&follower, "sub_b").await.as_deref(),
        Some("pending_durable")
    );
}

// ---------------------------------------------------------------------------
// B3 — terminal marking retries through REAL flush faults
// ---------------------------------------------------------------------------

/// Source whose pull reply carries a WRONG repo lineage (identity fence
/// trips `IdentityMismatch` on every pull; hello delegates untouched) — the
/// epoch-order recipe from `identity_fence/tests/epoch_order_tests.rs`.
struct WrongIdentitySource {
    inner: InProcessReplSource,
    bad_pull: ReplResponse,
}

#[async_trait]
impl ReplSource for WrongIdentitySource {
    async fn hello(
        &self,
        node_id: &str,
    ) -> Result<shamir_query_types::wire::repl::ReplResponse, ReplError> {
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        _db: &str,
        _repo: &str,
        _from_version: u64,
        _limit: u32,
        _wait_ms: Option<u32>,
    ) -> Result<shamir_query_types::wire::repl::ReplResponse, ReplError> {
        Ok(self.bad_pull.clone())
    }
}

/// COUNTERFACTUAL: reverting `mark_with_retry` to fail-and-log leaves the
/// row `active` (the flushed marking never lands while the seam fails) →
/// the `resync_required` assertions go red.
///
/// Determinism: `tokio::time::pause` (start_paused) auto-advances the
/// production retry sleeps on the current-thread runtime; the fence trips on
/// the mutated pull BEFORE decode/apply.
#[tokio::test(start_paused = true)]
async fn terminal_marking_retries_until_success() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;
    await_journal_ready(&leader, "main", version).await;

    // Build the WRONG-identity pull reply from the real leader (identity
    // regression via the source — the epoch_order recipe).
    let inner = InProcessReplSource::new((*leader).clone());
    let mut bad_pull = inner.pull("app", "main", 1, 1000, Some(0)).await.unwrap();
    let expected = leader
        .get_db("app")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .repo_identity()
        .await
        .unwrap();
    if let ReplResponse::Pull { repo_lineage_id, .. } = &mut bad_pull {
        let mut observed = expected.clone();
        observed.lineage_id[0] ^= 1;
        *repo_lineage_id = Some(observed.lineage_id);
    }

    // The factory hands loop#1 the HONEST source and every later start the
    // WRONG-identity source.
    let counter = Arc::new(AtomicUsize::new(0));
    let factory: ReplSourceFactory = {
        let leader = leader.clone();
        let bad_pull = bad_pull.clone();
        let counter = counter.clone();
        Arc::new(move |_sub: &Subscription| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
            } else {
                Arc::new(WrongIdentitySource {
                    inner: InProcessReplSource::new((*leader).clone()),
                    bad_pull: bad_pull.clone(),
                }) as Arc<dyn ReplSource>
            }
        })
    };

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1")
        .with_max_iterations(1);

    // loop#1: applies v1, exits, self-deregisters, lease invalidated.
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await;
    assert!(bookmark(&follower, "main").await >= version);

    // Arm the REAL seam: the next TWO flushes fail, then pass. Budget:
    // reconcile#2 barrier (fail), reconcile#3 barrier (fail), reconcile#4
    // barrier (pass → loop#2 spawns and trips the fence), marking attempt 1
    // (fail), attempt 2 (fail), attempt 3 (pass).
    let fault = SeedCatalogueFlushFaultForTest::arm_fail_first(&follower, 2);

    sup.reconcile().await; // barrier fail #1 — deferred
    assert!(!sup.is_running("sub_a"));
    sup.reconcile().await; // barrier fail #2 — deferred
    assert!(!sup.is_running("sub_a"));
    sup.reconcile().await; // barrier passes → loop#2 trips the fence
    sup.wait_for_subscription("sub_a").await; // joins loop#2 + its marking

    drop(fault);
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("resync_required"),
        "the retried terminal marking must have landed"
    );
    assert!(
        follower
            .has_resync_required_subscription_covering("app", "main")
            .await
            .unwrap(),
        "the quarantine must cover app/main"
    );
}
