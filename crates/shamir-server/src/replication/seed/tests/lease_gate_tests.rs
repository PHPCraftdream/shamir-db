//! S5c-X round 5 — supervisor lease/barrier tests (W3); round-6 item 7
//! test-oracle fixes.
//! Fixtures replicate `supervisor_tests.rs` locally (leader/follower
//! `build_db`, builder-batch catalogue writes, a supervisor factory) —
//! nothing is imported from that module. All synchronisation is Notify-,
//! join-, gate- and bounded-yield-based: `wait_for_subscription` joins the
//! loop tasks, the applied-oracle is the FOLLOWER's bookmark reaching the
//! captured leader version (`await_journal_progress` is COMPLETION progress,
//! NOT journal persistence — `repo_instance.rs::await_journal_progress`),
//! real catalogue seams (`SeedCatalogueFlushFaultForTest`, the lease
//! `apply_gate`, the final-write seam) block real mutators, and
//! `tokio::time::pause` advances the B3 retry sleeps. NO sleeps and NO
//! unbounded waits anywhere in this file. Scenarios:
//!   * T1 — create → reconcile starts a fresh subscription.
//!   * T2 — a REAL catalogue-flush fault at the start barrier defers the
//!     start (nothing spawned, nothing marked, lease invalidated); the
//!     retrying reconcile starts it.
//!   * T3 — a REAL flush fault fails the PAUSE of a LIVE follower: the row
//!     reads `paused`, the lease is invalid, the loop is stopped by the
//!     system (SYSTEM-driven deregistration, item-4 teardown), and reconcile
//!     never restarts a paused row.
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
//!
//!   (Round 6: the B3 terminal-retry test moved to `terminal_retry_tests.rs`;
//!   item-3/item-4 counterfactuals live in `startup_lease_tests.rs` and
//!   `multi_target_exit_tests.rs`.)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::poll; // poll! re-export
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
use tokio::sync::Notify;

/// A boxed follower-catalogue batch future (`poll!`-able / droppable mid-flight).
type BoxedBatch =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<BatchResponse, BatchError>> + Send>>;

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
pub(crate) async fn build_db() -> Arc<ShamirDb> {
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
/// helper). Callers capture `leader_version` and use the FOLLOWER's bookmark
/// coverage of it as the applied-oracle (round-6 item 7d):
/// `await_journal_progress` is completion progress, NOT journal durability.
pub(crate) async fn write_rows(leader: &ShamirDb, base: usize, n: usize) {
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
pub(crate) async fn leader_version(leader: &ShamirDb) -> u64 {
    leader
        .current_commit_version("app", "main")
        .await
        .expect("leader version")
}

/// Notify-based COMPLETION-progress wait: parks until the repo's completion
/// prefix covers `version` (round-6 item 7d: this is completion progress,
/// NOT a journal-persistence confirmation — see
/// `repo_instance.rs::await_journal_progress`). Used only where a live loop's
/// having APPLIED is the oracle (its bookmark is checked separately).
async fn await_journal_ready(db: &ShamirDb, repo: &str, version: u64) {
    db.get_db("app")
        .and_then(|d| d.get_repo(repo))
        .expect("repo exists")
        .await_journal_progress(version)
        .await
        .expect("journal readiness");
}

/// Follower `app/{repo}` replication bookmark.
pub(crate) async fn bookmark(follower: &ShamirDb, repo: &str) -> u64 {
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
pub(crate) async fn create_profile_and_subscription(follower: &ShamirDb) {
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
pub(crate) async fn run_op(follower: &ShamirDb, op: BatchOp) {
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
pub(crate) fn supervisor(follower: Arc<ShamirDb>, leader: Arc<ShamirDb>) -> SubscriptionSupervisor {
    let factory: ReplSourceFactory = Arc::new(move |_sub: &Subscription| {
        Arc::new(InProcessReplSource::new((*leader).clone())) as Arc<dyn ReplSource>
    });
    SubscriptionSupervisor::new(follower, factory, "follower-1").with_poll_wait_ms(2000)
}

/// Raw catalogue read of one subscription row (mirrors
/// `supervisor.rs::read_all`: the FilterContext is built engine-side).
pub(crate) async fn sub_row(follower: &ShamirDb, name: &str) -> Option<QueryValue> {
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

pub(crate) async fn sub_state(follower: &ShamirDb, name: &str) -> Option<String> {
    sub_row(follower, name)
        .await
        .and_then(|r| r.get("state").and_then(|v| v.as_str()).map(String::from))
}

pub(crate) async fn sub_generation(follower: &ShamirDb, name: &str) -> u64 {
    sub_row(follower, name)
        .await
        .and_then(|r| r.get("binding_generation").and_then(|v| v.as_u64()))
        .expect("row carries a binding generation")
}

// ---------------------------------------------------------------------------
// Bounded wait helpers (round-6 item 7)
// ---------------------------------------------------------------------------

/// Bounded yield-wait: NO sleeps — yields the current-thread runtime until
/// `done` holds; panics after the yield budget so a real hang fails loudly
/// instead of stalling CI.
pub(crate) async fn wait_until(mut done: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if done() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("wait_until: condition not reached within the yield budget");
}

/// Round-6 item 7d applied-oracle: the FOLLOWER's bookmark of `app/{repo}`
/// reaches the captured leader `version`. This is the REAL readiness
/// condition for a live loop — the old leader-side `await_journal_ready`
/// claim was completion progress, not journal persistence, and said nothing
/// about the follower.
pub(crate) async fn wait_until_bookmark_covers(follower: &ShamirDb, repo: &str, version: u64) {
    for _ in 0..10_000 {
        if bookmark(follower, repo).await >= version {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("follower bookmark of app/{repo} never reached v{version}");
}

/// SYSTEM-driven deregistration: bounded wait for the loop's own exit plus
/// the item-4 teardown to remove the registry entry — no manual handle
/// removal (`wait_for_subscription` removes the entry itself and must not be
/// used as a stop oracle).
pub(crate) async fn wait_until_deregistered(sup: &SubscriptionSupervisor, name: &str) {
    wait_until(|| !sup.is_running(name)).await;
}

/// Bounded wait until no lease exists for `name` (round-6 item 3: the RAII
/// startup-lease cleanup runs on a detached task).
pub(crate) async fn wait_until_lease_gone(follower: &ShamirDb, name: &str) {
    wait_until(|| follower.subscription_lease(name).is_none()).await;
}

// ---------------------------------------------------------------------------
// T1 — create → reconcile starts a fresh subscription
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_then_reconcile_starts_fresh_subscription() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let version = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // No faults — the fresh subscription MUST start on the first reconcile.
    // (Round-6 item 7d: the leader version is captured BEFORE the reconcile;
    // readiness is the follower's bookmark covering it, asserted below —
    // not a leader-side `await_journal_ready` claim.)
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
    // Round-7 item 7: the RAII `StartupLeaseGuard::Drop` cleanup is ASYNC
    // (it spawns a detached task — `seed/startup_lease.rs`), so the lease
    // removal must be awaited with the bounded helper, never asserted
    // synchronously.
    wait_until_lease_gone(&follower, "sub_a").await;

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
/// would stay valid and the loop would keep long-polling (the loop never
/// exits → `wait_until_deregistered` panics → red); without W2's exit-path
/// lease invalidation the later restart could not be taken (covered by the
/// T1/A1 tests' lease assertions).
/// Round-6 item 7(c): the applied-oracle is the FOLLOWER's bookmark covering
/// the captured leader version (bounded wait), and deregistration is awaited
/// SYSTEM-driven (loop's own exit + item-4 teardown) — no manual handle
/// removal via `wait_for_subscription`. The long-poll budget is small so the
/// parked pull returns promptly after the lease goes invalid.
#[tokio::test]
async fn pause_flush_failure_stops_live_follower_and_never_restarts() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Run-forever supervisor: the loop stays LIVE, long-polling.
    let sup = supervisor(follower.clone(), leader.clone()).with_poll_wait_ms(100);
    sup.reconcile().await;
    assert!(sup.is_running("sub_a"), "the loop must be live");

    // The follower APPLIED the captured leader version (real applied-oracle,
    // bounded wait — not a leader-side readiness claim).
    wait_until_bookmark_covers(&follower, "main", version).await;

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

    // The loop is stopped BY THE SYSTEM: bounded wait for the loop's own
    // exit + the exit-path teardown to deregister the handle (round-6 item
    // 7(c) — never the manual-removal helper).
    wait_until_deregistered(&sup, "sub_a").await;
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

/// COUNTERFACTUAL (round-7 item 4a): reverting the SYSTEM-driven exit-path
/// deregistration / stale-handle sweep leaves the dead (invalidated-lease)
/// handle registered → the start phase skips the name
/// (`registry.contains_sync`) → the loop is never restarted → the bookmark
/// is stuck at v1 and never reaches v2 → red. (Round-6 version used
/// `with_max_iterations(1)` + a manual `wait_for_subscription` registry
/// removal, which never exercised the supervisor's own stale-handle path.)
#[tokio::test]
async fn pause_resume_back_to_back_restarts_and_applies_new_event() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let v1 = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // Run-forever supervisor: the loop stays LIVE (no iteration cap). The
    // short long-poll budget lets the superseded loop return promptly after
    // the pause's quiesce invalidates its lease (T3's recipe).
    let sup = supervisor(follower.clone(), leader.clone()).with_poll_wait_ms(100);
    sup.reconcile().await;
    assert!(sup.is_running("sub_a"), "the loop must be live");
    // Round-7 item 4a applied-oracle: bounded wait for the follower's
    // bookmark covering v1 (never a manual registry touch).
    wait_until_bookmark_covers(&follower, "main", v1).await;
    let gen = sub_generation(&follower, "sub_a").await;

    // PAUSE the LIVE follower (real batch): the quiesce invalidates the
    // lease, the loop exits on its own (top-of-iteration lease check), and
    // its exit-path teardown deregisters — SYSTEM-driven, observed via the
    // bounded wait (NEVER the manual-removal helper `wait_for_subscription`).
    pause_op_future(follower.clone()).await.expect("pause");
    wait_until_deregistered(&sup, "sub_a").await;

    // RESUME back-to-back: the row ends active with the SAME generation.
    op_future(
        follower.clone(),
        shamir_query_builder::ddl::alter_subscription("sub_a")
            .resume()
            .build()
            .unwrap(),
    )
    .await
    .expect("resume");
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active")
    );
    assert_eq!(
        sub_generation(&follower, "sub_a").await,
        gen,
        "pause/resume must not bump the binding generation"
    );

    // NEW leader rows BEFORE the restarting reconcile; the restarted loop's
    // bookmark covering v2 is the applied-oracle (round-6 item 7d).
    write_rows(&leader, 10, 2).await;
    let v2 = leader_version(&leader).await;

    // THE SAME supervisor's reconcile restarts the loop: its stale-handle
    // sweep (if the handle is still registered) or the start phase (after
    // the SYSTEM-driven self-deregistration above) starts it under a FRESH
    // lease. Bounded wait — the loop runs forever.
    sup.reconcile().await;
    wait_until_bookmark_covers(&follower, "main", v2).await;
    assert!(
        sup.is_running("sub_a"),
        "the restarted loop must be live under its fresh lease"
    );
}

/// A CANCELLED quiesce — the PAUSE future dropped while its write gate is
/// blocked — leaves the row active and the lease invalid; the SAME
/// supervisor's stale-handle sweep then restarts the follower under a FRESH
/// lease (round-7 item 4b: ONE run-forever supervisor throughout — the
/// round-6 version's `stop_all` + new-supervisor restart never exercised
/// the sweep).
///
/// COUNTERFACTUAL: reverting the stale sweep → the invalidated handle stays
/// registered → reconcile's start phase skips the name
/// (`registry.contains_sync`) → no restart → the bookmark is stuck at v1
/// (never reaches v2) → red.
#[tokio::test]
async fn cancelled_quiesce_leaves_active_row_recoverable() {
    let leader = build_db().await;
    write_rows(&leader, 0, 3).await;
    let v1 = leader_version(&leader).await;
    await_journal_ready(&leader, "main", v1).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // ONE run-forever supervisor for the whole test: loop#1 stays LIVE
    // (long-polling between iterations) while the quiesce is cancelled.
    let sup = supervisor(follower.clone(), leader.clone());
    sup.reconcile().await;
    assert!(sup.is_running("sub_a"), "loop#1 must be live");
    // Deterministic: loop#1 applied v1 (bounded follower-side applied-oracle).
    wait_until_bookmark_covers(&follower, "main", v1).await;

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
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("active")
    );
    assert_eq!(
        sub_generation(&follower, "sub_a").await,
        gen,
        "resume must not bump the binding generation"
    );

    // NEW leader rows BEFORE the restarting reconcile.
    write_rows(&leader, 10, 2).await;
    let v2 = leader_version(&leader).await;

    // Round-7 item 4b: reconcile ON THE SAME supervisor. Loop#1 is still
    // long-polling on its (now invalid) OLD lease — typically still parked
    // mid-pull here, so the STALE-HANDLE SWEEP (lease superseded) is the
    // path that deregisters it and frees the start phase to restart under a
    // FRESH lease. Bounded applied-oracle: the restarted loop's bookmark
    // covering v2.
    sup.reconcile().await;
    wait_until_bookmark_covers(&follower, "main", v2).await;
    assert!(
        sup.is_running("sub_a"),
        "the restarted loop must be live under its FRESH lease"
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
    let sup_b =
        SubscriptionSupervisor::new(follower.clone(), factory, "follower-b").with_max_iterations(1);

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
        matches!(poll!(drop_future.as_mut()), std::task::Poll::Pending),
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
    // nothing spawns for sub_b and no lease is ever issued for it, while the
    // ACTIVE sub_a legitimately starts (bounded supervisor: the entry is
    // registered synchronously at the end of `start_subscription`).
    // Round-6 item 7(b): the old `active_count() == 0` oracle was WRONG —
    // reconcile legitimately starts active sub_a; the oracles are sub_b-
    // specific instead.
    let sup = supervisor(follower.clone(), leader).with_max_iterations(1);
    sup.reconcile().await;
    assert!(!sup.is_running("sub_b"), "a pending row must never start");
    assert!(
        sup.is_running("sub_a"),
        "the ACTIVE row legitimately starts"
    );
    assert_eq!(
        sup.active_count(),
        1,
        "exactly the active sub_a is registered"
    );
    assert!(
        follower.subscription_lease("sub_b").is_none(),
        "no lease may ever be issued for a pending row"
    );
    // app/main carries only sub_a's legit start — there are no leader rows,
    // so nothing was applied (bookmark 0) and sub_b stays pending_durable.
    assert_eq!(bookmark(&follower, "main").await, 0);
    assert_eq!(
        sub_state(&follower, "sub_b").await.as_deref(),
        Some("pending_durable")
    );
}
