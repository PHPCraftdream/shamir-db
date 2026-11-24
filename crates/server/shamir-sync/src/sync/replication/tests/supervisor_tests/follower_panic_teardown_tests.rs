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
use crate::replication::follower_loop::ApplyParkHook;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use crate::replication::supervisor::{ReplSourceFactory, Subscription, SubscriptionSupervisor};

use super::{build_db, create_profile_and_subscription};

struct PayloadDropPanics;

impl Drop for PayloadDropPanics {
    fn drop(&mut self) {
        panic!("payload drop panicked");
    }
}

struct PanicFixture {
    main_pull_entered: Arc<Notify>,
    release_main_pull: Arc<Notify>,
    sibling_parked: Arc<Notify>,
    release_sibling: Arc<Notify>,
    gap_served: Arc<Notify>,
    panic_hello: bool,
    sibling_gap: bool,
}

struct PanicTeardownSource {
    inner: InProcessReplSource,
    main_pull_entered: Arc<Notify>,
    release_main_pull: Arc<Notify>,
    sibling_parked: Arc<Notify>,
    release_sibling: Arc<Notify>,
    gap_served: Arc<Notify>,
    panic_hello: bool,
    sibling_gap: bool,
}

#[async_trait]
impl ReplSource for PanicTeardownSource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        if self.panic_hello {
            std::panic::panic_any(PayloadDropPanics);
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
                self.gap_served.notify_one();
                return Err(ReplError::JournalGap {
                    gap_at: 2,
                    from_version,
                });
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
            panic_hello: fixture.panic_hello,
            sibling_gap: fixture.sibling_gap,
        }) as Arc<dyn ReplSource>
    })
}

/// Mutant: without nested payload-disposal catch, `PayloadDropPanics::drop`
/// unwinds past wrapper bookkeeping and teardown never completes; this is
/// parent-red because the committed parent drops the payload bare.
#[tokio::test]
async fn one_target_hello_panic_payload_disposal_runs_teardown_once() {
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
    let factory = panic_factory(
        &leader,
        PanicFixture {
            main_pull_entered: Arc::new(Notify::new()),
            release_main_pull: Arc::new(Notify::new()),
            sibling_parked: Arc::new(Notify::new()),
            release_sibling: Arc::new(Notify::new()),
            gap_served: Arc::new(Notify::new()),
            panic_hello: true,
            sibling_gap: false,
        },
    );
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    let teardown_done = sup.teardown_signal();
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("reconcile bounded");
    tokio::time::timeout(Duration::from_secs(120), teardown_done.notified())
        .await
        .expect("panic wrapper must finish teardown");
    assert_eq!(sup.teardown_count(), 1);
    assert!(follower.subscription_lease("sub_a").is_none());
    assert!(!sup.is_running("sub_a"));
}

/// Mutants killed: (a) unwinding past the wrapper (no catch_unwind) means
/// teardown never runs; (b) missing sibling-cancel on panic means the sibling
/// never exits and bounded teardown fails; (c) teardown-at-panicking-exit
/// fails the mid-test zero-count assertion; (d) a token-releasable sibling
/// park reaches teardown before that assertion. The sibling's pull gate is a
/// plain Notify and cannot be released by either cancellation token. If the
/// apply park is reached after release_sibling, the loops token is already
/// cancelled, so it returns immediately and cannot block the sibling's exit.
#[tokio::test]
async fn two_target_pull_panic_waits_for_both_wrappers_before_single_teardown() {
    let leader = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("leader setup bounded");
    let follower = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("follower setup bounded");
    tokio::time::timeout(
        Duration::from_secs(120),
        create_two_target_subscription(&follower),
    )
    .await
    .expect("subscription setup bounded");
    let main_pull_entered = Arc::new(Notify::new());
    let release_main_pull = Arc::new(Notify::new());
    let sibling_parked = Arc::new(Notify::new());
    let release_sibling = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let factory = panic_factory(
        &leader,
        PanicFixture {
            main_pull_entered: Arc::clone(&main_pull_entered),
            release_main_pull: Arc::clone(&release_main_pull),
            sibling_parked: Arc::clone(&sibling_parked),
            release_sibling: Arc::clone(&release_sibling),
            gap_served: Arc::new(Notify::new()),
            panic_hello: false,
            sibling_gap: false,
        },
    );
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1").with_apply_park(
        Arc::new(ApplyParkHook {
            repo: Some("other".into()),
            after_apply: false,
            arrived: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            exited: Arc::clone(&exited),
        }),
    );
    let teardown_done = sup.teardown_signal();
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("reconcile bounded");
    tokio::time::timeout(Duration::from_secs(120), main_pull_entered.notified())
        .await
        .expect("main pull must park");
    tokio::time::timeout(Duration::from_secs(120), sibling_parked.notified())
        .await
        .expect("sibling pull must park");
    release_main_pull.notify_one();
    tokio::time::timeout(Duration::from_secs(120), exited.notified())
        .await
        .expect("panicking wrapper exit must be observed");
    assert_eq!(sup.teardown_count(), 0, "parked sibling holds live count");
    release_sibling.notify_one();
    tokio::time::timeout(Duration::from_secs(120), teardown_done.notified())
        .await
        .expect("both wrappers must finish before teardown");
    assert_eq!(sup.teardown_count(), 1);
    assert!(follower.subscription_lease("sub_a").is_none());
    assert!(!sup.is_running("sub_a"));
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("restart reconcile bounded");
    assert!(sup.is_running("sub_a"));
}

/// The terminal mark observes the parent `cancel` token, not `loops_cancel`;
/// therefore sibling cancellation cannot abort its durable `resync_required`
/// mark. Parent-red: cancelling the terminal-observed token on panic leaves
/// this row active instead of resync_required.
#[tokio::test]
async fn a_sibling_terminal_mark_survives_a_concurrent_panic() {
    let leader = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("leader setup bounded");
    let follower = tokio::time::timeout(Duration::from_secs(120), build_two_repo_db())
        .await
        .expect("follower setup bounded");
    tokio::time::timeout(
        Duration::from_secs(120),
        create_two_target_subscription(&follower),
    )
    .await
    .expect("subscription setup bounded");
    let gap_served = Arc::new(Notify::new());
    let main_pull_entered = Arc::new(Notify::new());
    let release_main_pull = Arc::new(Notify::new());
    let factory = panic_factory(
        &leader,
        PanicFixture {
            main_pull_entered: Arc::clone(&main_pull_entered),
            release_main_pull: Arc::clone(&release_main_pull),
            sibling_parked: Arc::new(Notify::new()),
            release_sibling: Arc::new(Notify::new()),
            gap_served: Arc::clone(&gap_served),
            panic_hello: false,
            sibling_gap: true,
        },
    );
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    let teardown_done = sup.teardown_signal();
    tokio::time::timeout(Duration::from_secs(120), sup.reconcile())
        .await
        .expect("reconcile bounded");
    tokio::time::timeout(Duration::from_secs(120), main_pull_entered.notified())
        .await
        .expect("main pull must park");
    tokio::time::timeout(Duration::from_secs(120), gap_served.notified())
        .await
        .expect("sibling must serve JournalGap");
    release_main_pull.notify_one();
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let rows = sup
                .read_subscriptions()
                .await
                .expect("read durable subscriptions");
            if rows
                .iter()
                .any(|row| row.name == "sub_a" && row.state == "resync_required")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("sibling terminal mark must become durable");
    tokio::time::timeout(Duration::from_secs(120), teardown_done.notified())
        .await
        .expect("shared teardown must finish");
    assert_eq!(sup.teardown_count(), 1);
}
