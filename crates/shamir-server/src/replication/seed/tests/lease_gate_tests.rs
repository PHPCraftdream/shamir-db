//! S5c-X round 4 — supervisor lease/barrier tests (W3).
//!
//! Fixtures replicate `supervisor_tests.rs` locally (leader/follower
//! `build_db`, builder-batch catalogue writes, a supervisor factory) —
//! nothing is imported from that module. All synchronisation is Notify- and
//! join-based (`wait_for_subscription` joins the loop tasks): NO sleeps and
//! NO yield budgets anywhere in this file.
//!
//! Scenarios:
//!   * T1 — create → reconcile starts a FRESH subscription (the case the
//!     round-3 wrap bug broke): follower bookmark reaches the leader.
//!   * T2 — an injected start-barrier failure defers the start (nothing
//!     marked, nothing spawned); the retrying reconcile starts it.
//!   * T3 — PAUSE deregisters the follower; reconcile never restarts a
//!     paused row.
//!   * T4 — an obsolete G1 loop parked mid-Pull applies nothing after the
//!     binding was replaced by a strictly-greater G2.
//!   * T5 — the start-race hook replaces the row between lease-take and
//!     spawn: the start is dropped, nothing spawns, nothing is marked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{drop_subscription, repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use tokio::sync::Notify;

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
/// helper). The source's long-poll budget covers journal durability, so no
/// polling loop is needed here.
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

/// Run one builder op against the follower catalogue.
async fn run_op(follower: &ShamirDb, op: shamir_query_types::batch::BatchOp) {
    let mut b = Batch::new();
    b.id(1);
    b.op("op", op);
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("catalogue batch");
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

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    // No barrier faults — the fresh subscription MUST start on the first
    // reconcile (the case the round-3 fault-counter wrap broke).
    let sup = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await; // joins the loop

    assert!(
        bookmark(&follower, "main").await >= version,
        "the fresh subscription must have started and caught up"
    );
}

// ---------------------------------------------------------------------------
// T2 — start-barrier failure defers the start; retry starts it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_barrier_failure_defers_start_until_retry() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone())
        .with_max_iterations(1)
        .with_start_barrier_faults(1);

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

    // The retry (counter exhausted → real flush) starts the loop.
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await;
    assert!(
        bookmark(&follower, "main").await >= version,
        "the retried reconcile must start the subscription"
    );
}

// ---------------------------------------------------------------------------
// T3 — pause deregisters; reconcile never restarts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_deregisters_follower_and_reconcile_never_restarts() {
    let leader = build_db().await;
    write_rows(&leader, 0, 2).await;
    let version = leader_version(&leader).await;

    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;

    let sup = supervisor(follower.clone(), leader.clone()).with_max_iterations(1);
    sup.reconcile().await;
    sup.wait_for_subscription("sub_a").await; // loop applied — it WAS live
    assert!(bookmark(&follower, "main").await >= version);

    run_op(
        &follower,
        shamir_query_builder::ddl::alter_subscription("sub_a")
            .pause()
            .build()
            .unwrap(),
    )
    .await;
    sup.notify_changed().await;
    assert!(!sup.is_running("sub_a"), "the paused loop must be gone");
    assert_eq!(sup.active_count(), 0);

    // A paused row is never restarted by a further reconcile.
    sup.reconcile().await;
    assert_eq!(sup.active_count(), 0);
    assert_eq!(
        sub_state(&follower, "sub_a").await.as_deref(),
        Some("paused")
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

    // While parked: DROP sub_a — the quiesce finds no read guard and the
    // drop completes — then re-create it over prof_b (G2, strictly greater).
    run_op(&follower, drop_subscription("sub_a")).await;
    create_profile_b(&follower).await;
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_b"),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("re-create sub_a over prof_b");
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
                let mut b = Batch::new();
                b.id(2);
                b.op(
                    "cs",
                    subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_b"),
                );
                follower
                    .execute("app", &b.to_request_via_msgpack())
                    .await
                    .expect("re-create sub_a over prof_b");
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
