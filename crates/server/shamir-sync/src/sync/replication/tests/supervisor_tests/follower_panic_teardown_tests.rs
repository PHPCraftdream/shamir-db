//! Regression coverage for follower-loop panic teardown.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Notify;

use crate::replication::error::ReplError;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::seed::terminal;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{
    HousekeepingGate, ReplSourceFactory, Subscription, SubscriptionSupervisor,
};

use super::{build_db, create_profile_and_subscription};

struct PayloadDropPanics;
impl Drop for PayloadDropPanics {
    fn drop(&mut self) {
        panic!("payload drop panicked");
    }
}
struct InnerDropPanics;
impl Drop for InnerDropPanics {
    fn drop(&mut self) {
        panic!("inner drop panicked");
    }
}
struct OuterDropPanics;
impl Drop for OuterDropPanics {
    fn drop(&mut self) {
        std::panic::panic_any(InnerDropPanics);
    }
}

#[derive(Clone, Copy)]
enum PanicMode {
    None,
    DropPanics,
    NestedDropPanics,
}

struct PanicFixture {
    main_pull_entered: Arc<Notify>,
    release_main_pull: Arc<Notify>,
    sibling_parked: Arc<Notify>,
    release_sibling: Arc<Notify>,
    gap_served: Arc<Notify>,
    panic_mode: PanicMode,
    sibling_gap: bool,
}

struct PanicTeardownSource {
    inner: InProcessReplSource,
    main_pull_entered: Arc<Notify>,
    release_main_pull: Arc<Notify>,
    sibling_parked: Arc<Notify>,
    release_sibling: Arc<Notify>,
    gap_served: Arc<Notify>,
    panic_mode: PanicMode,
    sibling_gap: bool,
}

#[async_trait]
impl ReplSource for PanicTeardownSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        match self.panic_mode {
            PanicMode::DropPanics => std::panic::panic_any(PayloadDropPanics),
            PanicMode::NestedDropPanics => std::panic::panic_any(OuterDropPanics),
            PanicMode::None => {}
        }
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
        if repo == "main" {
            self.main_pull_entered.notify_one();
            self.release_main_pull.notified().await;
            panic!("intentional main-pull panic for teardown regression");
        }
        if repo == "other" {
            if self.sibling_gap {
                // The leader's real identity and epoch keep the identity fence
                // passing, so the loop reaches the gap stop.
                let mut reply = self
                    .inner
                    .pull(db, repo, from_version, limit, Some(0))
                    .await?;
                if let ReplResponse::Pull { gap_at, .. } = &mut reply {
                    *gap_at = Some(from_version);
                }
                self.gap_served.notify_one();
                return Ok(reply);
            }
            self.sibling_parked.notify_one();
            self.release_sibling.notified().await;
            return self
                .inner
                .pull(db, repo, from_version, limit, wait_ms)
                .await;
        }
        self.inner
            .pull(db, repo, from_version, limit, wait_ms)
            .await
    }
}

async fn build_two_repo_db() -> Arc<ShamirDb> {
    let db = ShamirDb::init_memory().await.expect("init database");
    let owner = Actor::User(principal64_from_username("alice"));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create app");
    for repo in ["main", "other"] {
        let config =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        db.add_repo_as("app", config, owner.clone())
            .await
            .expect("create repo");
    }
    Arc::new(db)
}

async fn create_two_target_subscription(follower: &ShamirDb) {
    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "cp",
        replication_profile("prof_ab")
            .stream(
                repl_scope("app").repo("main").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            )
            .stream(
                repl_scope("app").repo("other").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            ),
    );
    batch.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "prof_ab"),
    );
    follower
        .execute(
            "app",
            &batch.to_request_via_msgpack().expect("encode batch"),
        )
        .await
        .expect("create two-target subscription");
}

fn panic_factory(leader: &Arc<ShamirDb>, fixture: PanicFixture) -> ReplSourceFactory {
    let leader = Arc::clone(leader);
    Arc::new(move |_sub: &Subscription| {
        Arc::new(PanicTeardownSource {
            inner: InProcessReplSource::new((*leader).clone()),
            main_pull_entered: Arc::clone(&fixture.main_pull_entered),
            release_main_pull: Arc::clone(&fixture.release_main_pull),
            sibling_parked: Arc::clone(&fixture.sibling_parked),
            release_sibling: Arc::clone(&fixture.release_sibling),
            gap_served: Arc::clone(&fixture.gap_served),
            panic_mode: fixture.panic_mode,
            sibling_gap: fixture.sibling_gap,
        }) as Arc<dyn ReplSource>
    })
}

fn fixture(mode: PanicMode, sibling_gap: bool) -> PanicFixture {
    PanicFixture {
        main_pull_entered: Arc::new(Notify::new()),
        release_main_pull: Arc::new(Notify::new()),
        sibling_parked: Arc::new(Notify::new()),
        release_sibling: Arc::new(Notify::new()),
        gap_served: Arc::new(Notify::new()),
        panic_mode: mode,
        sibling_gap,
    }
}

async fn assert_active(sup: &SubscriptionSupervisor) {
    let rows = sup
        .read_subscriptions()
        .await
        .expect("read durable subscriptions");
    assert!(rows
        .iter()
        .any(|row| row.name == "sub_a" && row.state == "active"));
}

/// Nested disposal must not unwind past bookkeeping; parent-red before the second-payload leak guard.
#[tokio::test]
async fn one_target_hello_panic_payload_disposal_runs_teardown_once() {
    for mode in [PanicMode::DropPanics, PanicMode::NestedDropPanics] {
        let follower = tokio::time::timeout(Duration::from_secs(120), build_db())
            .await
            .expect("follower setup bounded");
        tokio::time::timeout(
            Duration::from_secs(120),
            create_profile_and_subscription(&follower),
        )
        .await
        .expect("subscription setup bounded");
        let leader = tokio::time::timeout(Duration::from_secs(120), build_db())
            .await
            .expect("leader setup bounded");
        let sup = SubscriptionSupervisor::new(
            follower.clone(),
            panic_factory(&leader, fixture(mode, false)),
            "follower-1",
        );
        let teardown = sup.teardown_signal();
        tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
            .await
            .expect("reconcile bounded");
        tokio::time::timeout(Duration::from_secs(120), teardown.notified())
            .await
            .expect("panic teardown bounded");
        assert_eq!(sup.teardown_count(), 1);
        assert!(follower.subscription_lease("sub_a").is_none());
        assert!(!sup.is_running("sub_a"));
        assert_active(&sup).await;
    }
}

/// The sibling reaches pull cancellation and then housekeeping; teardown waits for its decrement.
#[tokio::test]
async fn two_target_pull_panic_waits_for_both_wrappers_before_single_teardown() {
    let leader = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("leader bounded");
    let follower = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("follower bounded");
    tokio::time::timeout(
        Duration::from_secs(120),
        create_two_target_subscription(&follower),
    )
    .await
    .expect("subscription bounded");
    let f = fixture(PanicMode::None, false);
    let gate = Arc::new(HousekeepingGate {
        repo: Some("other".into()),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let sup = SubscriptionSupervisor::new(
        follower.clone(),
        panic_factory(
            &leader,
            PanicFixture {
                main_pull_entered: Arc::clone(&f.main_pull_entered),
                release_main_pull: Arc::clone(&f.release_main_pull),
                sibling_parked: Arc::clone(&f.sibling_parked),
                release_sibling: Arc::clone(&f.release_sibling),
                gap_served: Arc::clone(&f.gap_served),
                panic_mode: PanicMode::None,
                sibling_gap: false,
            },
        ),
        "follower-1",
    )
    .with_housekeeping_gate(Arc::clone(&gate));
    let accounted = sup.accounted_signal();
    let teardown = sup.teardown_signal();
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("reconcile bounded");
    tokio::time::timeout(Duration::from_secs(120), f.main_pull_entered.notified())
        .await
        .expect("main pull bounded");
    tokio::time::timeout(Duration::from_secs(120), f.sibling_parked.notified())
        .await
        .expect("sibling pull bounded");
    f.release_main_pull.notify_one();
    tokio::time::timeout(Duration::from_secs(120), async {
        tokio::join!(accounted.notified(), gate.entered.notified());
    })
    .await
    .expect("main accounting and sibling housekeeping bounded");
    assert_eq!(sup.teardown_count(), 0);
    assert_eq!(
        follower
            .subscription_lease("sub_a")
            .map(|l| l.valid.load(std::sync::atomic::Ordering::Acquire)),
        Some(true),
        "lease must still be valid before the sibling's housekeeping"
    );
    assert!(sup.is_running("sub_a"));
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(120), teardown.notified())
        .await
        .expect("teardown bounded");
    assert_eq!(sup.teardown_count(), 1);
    assert!(follower.subscription_lease("sub_a").is_none());
    assert!(!sup.is_running("sub_a"));
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("restart reconcile bounded");
    assert!(sup.is_running("sub_a"));
}

/// Regression guard for token separation: parent already separates these tokens; a re-coupled mark cancellation mutant leaves completion unsignalled.
#[tokio::test]
async fn a_sibling_terminal_mark_survives_a_concurrent_panic() {
    let leader = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("leader bounded");
    let follower = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("follower bounded");
    tokio::time::timeout(
        Duration::from_secs(120),
        create_two_target_subscription(&follower),
    )
    .await
    .expect("subscription bounded");
    let f = fixture(PanicMode::None, true);
    let gate_guard = terminal::mark_gate_arm();
    let gate = Arc::clone(&gate_guard.gate);
    let sup = SubscriptionSupervisor::new(
        follower.clone(),
        panic_factory(
            &leader,
            PanicFixture {
                main_pull_entered: Arc::clone(&f.main_pull_entered),
                release_main_pull: Arc::clone(&f.release_main_pull),
                sibling_parked: Arc::clone(&f.sibling_parked),
                release_sibling: Arc::clone(&f.release_sibling),
                gap_served: Arc::clone(&f.gap_served),
                panic_mode: PanicMode::None,
                sibling_gap: true,
            },
        ),
        "follower-1",
    );
    let teardown = sup.teardown_signal();
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("reconcile bounded");
    tokio::time::timeout(Duration::from_secs(120), f.main_pull_entered.notified())
        .await
        .expect("main pull bounded");
    tokio::time::timeout(Duration::from_secs(120), f.gap_served.notified())
        .await
        .expect("gap reply bounded");
    tokio::time::timeout(Duration::from_secs(120), gate.entered.notified())
        .await
        .expect("mark gate bounded");
    f.release_main_pull.notify_one();
    tokio::time::timeout(Duration::from_secs(120), gate.loops_token.cancelled())
        .await
        .expect("loops cancellation bounded");
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(120), gate.completed.notified())
        .await
        .expect("mark completion bounded");
    let rows = sup
        .read_subscriptions()
        .await
        .expect("read durable subscriptions");
    assert!(rows
        .iter()
        .any(|row| row.name == "sub_a" && row.state == "resync_required"));
    tokio::time::timeout(Duration::from_secs(120), teardown.notified())
        .await
        .expect("teardown bounded");
    assert_eq!(sup.teardown_count(), 1);
}
