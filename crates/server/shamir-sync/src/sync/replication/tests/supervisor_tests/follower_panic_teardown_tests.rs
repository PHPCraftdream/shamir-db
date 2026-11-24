//! Regression coverage for follower-loop panic teardown.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::Insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Notify;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::ApplyParkHook;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

use super::{build_db, create_profile_and_subscription};

struct PanicHelloSource {
    entered: Arc<Notify>,
    armed: Arc<AtomicBool>,
    inner: InProcessReplSource,
}

#[async_trait]
impl ReplSource for PanicHelloSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        if self.armed.swap(false, Ordering::AcqRel) {
            self.entered.notify_one();
            panic!("intentional Hello panic for teardown regression");
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

async fn write_leader_rows(leader: &ShamirDb) {
    let owner = Actor::User(principal64_from_username("alice"));
    for repo in ["main", "other"] {
        let mut batch = Batch::named("ins");
        batch.id("ins");
        batch.transactional();
        batch.insert(
            "i",
            Insert::with_repo(repo, "items").rows([doc! { "id" => "row-1", "v" => 1_i64 }]),
        );
        leader
            .execute_as(owner.clone(), "app", &batch.build())
            .await
            .expect("leader fixture write");
    }
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

fn panic_factory(
    entered: Arc<Notify>,
    leader: &Arc<ShamirDb>,
    initially_armed: bool,
) -> (ReplSourceFactory, Arc<AtomicBool>) {
    let leader = Arc::clone(leader);
    let armed = Arc::new(AtomicBool::new(initially_armed));
    let factory_armed = Arc::clone(&armed);
    let factory = Arc::new(move |_sub: &Subscription| {
        Arc::new(PanicHelloSource {
            entered: Arc::clone(&entered),
            armed: Arc::clone(&factory_armed),
            inner: InProcessReplSource::new((*leader).clone()),
        }) as Arc<dyn ReplSource>
    });
    (factory, armed)
}

/// Mutant killed: (a) `inner-detachable-spawn` lets Hello's panic escape the
/// wrapper so teardown never advances.
#[tokio::test]
async fn one_target_hello_panic_runs_teardown_once_and_clears_lease_and_registry() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let entered = Arc::new(Notify::new());
    let leader = build_db().await;
    let (factory, _) = panic_factory(Arc::clone(&entered), &leader, true);
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    let teardown = sup.teardown_signal();
    let handshake = entered.notified();
    tokio::pin!(handshake);
    sup.reconcile().await;
    tokio::time::timeout(Duration::from_secs(120), &mut handshake)
        .await
        .expect("Hello must reach panic source");
    tokio::time::timeout(Duration::from_secs(120), teardown.notified())
        .await
        .expect("panic wrapper must finish teardown");
    assert_eq!(sup.teardown_count(), 1);
    assert!(follower.subscription_lease("sub_a").is_none());
    assert!(!sup.is_running("sub_a"));
    let rows = sup
        .read_subscriptions()
        .await
        .expect("read durable subscription");
    assert_eq!(
        rows.iter().find(|row| row.name == "sub_a").unwrap().state,
        "active"
    );
}

/// Mutants killed: (a) `inner-detachable-spawn` lets Hello's panic escape the
/// wrapper so teardown never advances; (b) missing sibling-cancel on the panic
/// path leaves the parked sibling running, so the bounded teardown wait fails;
/// (c) teardown-at-panicking-exit tears down before the parked sibling exits,
/// so the mid-test zero-count assertion fails. The panic path cancels the shared
/// token, so once the apply gate opens the sibling exits without a second pull.
#[tokio::test]
async fn two_target_one_hello_panic_waits_for_both_wrappers_before_single_teardown() {
    let leader = build_two_repo_db().await;
    write_leader_rows(&leader).await;
    let follower = build_two_repo_db().await;
    create_two_target_subscription(&follower).await;
    let entered = Arc::new(Notify::new());
    let (factory, armed) = panic_factory(Arc::clone(&entered), &leader, false);
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1").with_apply_park(
        Arc::new(ApplyParkHook {
            repo: Some("other".into()),
            after_apply: false,
            arrived: Arc::clone(&arrived),
            release: Arc::clone(&release),
            exited: Arc::clone(&exited),
        }),
    );
    let teardown = sup.teardown_signal();
    let teardown_done = teardown.notified();
    tokio::pin!(teardown_done);
    let panic_entered = entered.notified();
    tokio::pin!(panic_entered);
    let sibling_arrived = arrived.notified();
    tokio::pin!(sibling_arrived);
    sup.reconcile().await;
    tokio::time::timeout(Duration::from_secs(120), &mut sibling_arrived)
        .await
        .expect("other target must park at the apply gate");
    armed.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(120), &mut panic_entered)
        .await
        .expect("main target must enter the panic source");
    tokio::time::timeout(Duration::from_secs(120), exited.notified())
        .await
        .expect("main target wrapper must witness its exit");
    assert_eq!(
        sup.teardown_count(),
        0,
        "parked sibling holds the live count"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(120), &mut teardown_done)
        .await
        .expect("both wrappers must finish before shared teardown");
    assert_eq!(sup.teardown_count(), 1);
    assert!(follower.subscription_lease("sub_a").is_none());
    assert!(!sup.is_running("sub_a"));
}
