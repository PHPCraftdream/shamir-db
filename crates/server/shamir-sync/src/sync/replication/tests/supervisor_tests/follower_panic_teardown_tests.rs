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
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Notify;

use crate::replication::error::ReplError;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

use super::{build_db, create_profile_and_subscription};

struct PanicHelloSource {
    entered: Arc<Notify>,
    panic_next: Arc<AtomicBool>,
    inner: InProcessReplSource,
}

#[async_trait]
impl ReplSource for PanicHelloSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        if self.panic_next.swap(false, Ordering::AcqRel) {
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

fn panic_factory(entered: Arc<Notify>, leader: &Arc<ShamirDb>) -> ReplSourceFactory {
    let leader = Arc::clone(leader);
    let panic_next = Arc::new(AtomicBool::new(true));
    Arc::new(move |_sub: &Subscription| {
        Arc::new(PanicHelloSource {
            entered: Arc::clone(&entered),
            panic_next: Arc::clone(&panic_next),
            inner: InProcessReplSource::new((*leader).clone()),
        }) as Arc<dyn ReplSource>
    })
}

/// Mutant killed: remove the inner JoinHandle boundary and let Hello's panic unwind
/// the wrapper. Then teardown_count never advances, the lease remains valid, and
/// the registry entry remains present, so the bounded handshake wait/assertions fail.
#[tokio::test]
async fn one_target_hello_panic_runs_teardown_once_and_clears_lease_and_registry() {
    let follower = build_db().await;
    create_profile_and_subscription(&follower).await;
    let entered = Arc::new(Notify::new());
    let leader = build_db().await;
    let sup = SubscriptionSupervisor::new(
        follower.clone(),
        panic_factory(Arc::clone(&entered), &leader),
        "follower-1",
    );
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

/// Mutant killed: omit the live-count decrement after a loop panic. The healthy
/// sibling exits after one normal iteration, but teardown then never signals,
/// leaving the lease and registry entry uncleared.
#[tokio::test]
async fn two_target_one_hello_panic_waits_for_both_wrappers_before_single_teardown() {
    let leader = build_two_repo_db().await;
    let follower = build_two_repo_db().await;
    create_two_target_subscription(&follower).await;
    let entered = Arc::new(Notify::new());
    let factory = panic_factory(Arc::clone(&entered), &leader);
    let sup =
        SubscriptionSupervisor::new(follower.clone(), factory, "follower-1").with_max_iterations(1);
    let teardown = sup.teardown_signal();
    sup.reconcile().await;
    tokio::time::timeout(Duration::from_secs(120), entered.notified())
        .await
        .expect("panicking target must enter Hello");
    tokio::time::timeout(Duration::from_secs(120), teardown.notified())
        .await
        .expect("both wrappers must finish before shared teardown");
    assert_eq!(sup.teardown_count(), 1);
    assert!(follower.subscription_lease("sub_a").is_none());
    assert!(!sup.is_running("sub_a"));
}
