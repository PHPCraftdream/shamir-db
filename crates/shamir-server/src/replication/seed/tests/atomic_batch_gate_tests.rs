//! S5c-X PR6b-3 R7 — up-front atomic-batch capability gate (R7-2(i)) and
//! supervisor terminal persistence (R7-3 regressions 2 and 3).
//!
//! A destination repository whose backend is MemBuffer-wrapped cannot host
//! the receipt pipeline (`DbError::AtomicBatchUnsupported` from admission).
//! Before R7 the supervisor spawned the loop anyway and the follower spun
//! warn+backoff+retry forever (the round-6 CI stall). Now `start_subscription`
//! refuses BEFORE spawning and persists the typed terminal status.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

/// Build a follower `ShamirDb` whose `app/main` destination repo is
/// MemBuffer-wrapped (`Store::supports_atomic_batch() == false`).
async fn build_membuffer_follower() -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username("alice"));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg = RepoConfig::new(
        "main",
        BoxRepoFactory::membuffer(BoxRepoFactory::in_memory(), Default::default()),
    )
    .add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    Arc::new(shamir)
}

/// Create the replication profile + subscription bound to `app/main` on the
/// follower catalogue.
async fn create_profile_and_subscription(follower: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cp",
        replication_profile("prof_ab").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription(name, "tcp://leader:9000", "pub_ab", "prof_ab"),
    );
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("create profile+subscription");
}

async fn sub_row(follower: &ShamirDb, name: &str) -> (Option<String>, Option<String>) {
    let table = follower
        .system_store()
        .subscriptions_table()
        .await
        .expect("subscriptions table");
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = shamir_db::types::common::new_map();
    let ctx = shamir_db::query::filter::FilterContext::new(interner, &refs);
    let rows = table
        .read(
            &shamir_db::query::read::ReadQuery::new("subscriptions"),
            &ctx,
        )
        .await
        .expect("read subscriptions");
    for row in rows.records {
        let row = row.as_value();
        if row.get("name").and_then(|v| v.as_str()) == Some(name) {
            return (
                row.get("state").and_then(|v| v.as_str()).map(String::from),
                row.get("status_message")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            );
        }
    }
    (None, None)
}

/// R7-3 (2): an existing MemBuffer-wrapped destination bound by CREATE
/// SUBSCRIPTION / RESUME is refused UP FRONT with the typed terminal error.
///
/// COUNTERFACTUAL: without the up-front capability check, `start_subscription`
/// spawns the loop; its first apply trips `DbError::AtomicBatchUnsupported`
/// inside the receipt admission, which (pre-R7-2(ii)) was warn+backoff+retry —
/// the loop spins until the stall threshold. The first failing assertion would
/// be `assert!(!sup.is_running("gate_sub"))` (the loop WOULD be registered).
#[tokio::test]
async fn membuffer_destination_is_refused_up_front() {
    let follower = build_membuffer_follower().await;
    create_profile_and_subscription(&follower, "gate_sub").await;
    // The factory panics: a refusal must happen BEFORE any source is built.
    let factory: ReplSourceFactory =
        Arc::new(|_| panic!("must refuse destination before constructing source"));
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    sup.reconcile().await;

    assert!(
        !sup.is_running("gate_sub"),
        "an unsupported destination must be refused before spawning"
    );
    let (state, message) = sub_row(&follower, "gate_sub").await;
    assert_eq!(
        state.as_deref(),
        Some("resync_required"),
        "refusal must persist the typed terminal status"
    );
    let message = message.expect("refusal message must name the remedy");
    assert!(
        message.contains("MemBuffer-wrapped backend")
            && message.contains("re-create it as a raw/replica repository"),
        "unexpected status_message: {message}"
    );

    // RESUME takes the same admission path: reconcile of the still-active row
    // refuses again (no loop, terminal status persists) instead of spawning.
    sup.reconcile().await;
    assert!(!sup.is_running("gate_sub"), "resume must be refused too");
    assert_eq!(
        sub_row(&follower, "gate_sub").await.0.as_deref(),
        Some("resync_required"),
    );
    sup.stop_all().await;
}

/// R7-3 (3): supervisor start on an unsupported backend persists the terminal
/// status and does NOT spin — no loop is registered, no source is constructed,
/// and a second reconcile does not retry the start.
///
/// COUNTERFACTUAL: without the terminal persistence the row would stay
/// `active` and every reconcile would (re)spawn a spinning loop — the first
/// failing assertion is `sub_state == "resync_required"` below, and the
/// `calls` counter would grow past 0 as reconciles re-enter the factory.
#[tokio::test]
async fn unsupported_backend_persists_terminal_status_without_spinning() {
    let follower = build_membuffer_follower().await;
    create_profile_and_subscription(&follower, "gate_sub").await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let factory: ReplSourceFactory = Arc::new(move |_| {
        count.fetch_add(1, Ordering::AcqRel);
        panic!("unsupported backend must never reach the source factory")
    });
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    sup.reconcile().await;
    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "refusal must happen before any source construction"
    );
    assert!(!sup.is_running("gate_sub"), "no loop may be registered");
    let (state, message) = sub_row(&follower, "gate_sub").await;
    assert_eq!(
        state.as_deref(),
        Some("resync_required"),
        "terminal status must be persisted exactly once, up front"
    );
    assert!(
        message
            .as_deref()
            .is_some_and(|m| m.contains("MemBuffer-wrapped backend")),
        "unexpected status_message: {message:?}"
    );

    // Bounded-iteration / is_running observation: repeated reconciles neither
    // start a loop nor flip the row back to active (no tight retry).
    sup.reconcile().await;
    sup.reconcile().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!sup.is_running("gate_sub"));
    assert_eq!(calls.load(Ordering::Acquire), 0);
    let (state, _) = sub_row(&follower, "gate_sub").await;
    assert_eq!(state.as_deref(), Some("resync_required"));
    sup.stop_all().await;
}
