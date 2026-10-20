//! S5c-X PR6b-3 R7/R9 — up-front atomic-batch capability gate (R7-2(i)) and
//! supervisor terminal persistence (R7-3 regressions 2 and 3), plus the R9
//! CREATE/RESUME admission refusals.
//!
//! A destination repository whose backend is MemBuffer-wrapped (or Hybrid)
//! cannot host the receipt pipeline (`DbError::AtomicBatchUnsupported` from
//! admission). Before R7 the supervisor spawned the loop anyway and the
//! follower spun warn+backoff+retry forever (the round-6 CI stall). Now
//! CREATE SUBSCRIPTION and ALTER ... RESUME refuse UP FRONT with the typed
//! `atomic_batch_unsupported` code, and `start_subscription` refuses BEFORE
//! spawning and persists the typed terminal status.
//!
//! Legacy rows are injected through the REAL system-catalogue handle
//! (`SystemStore::inject_subscription_row_for_test`, i.e. `subscriptions_table()`
//! plus `set_via_implicit_tx`, no db name). A batch `Insert` addressed to db
//! `"system"` fails with `Database 'system' not found` because the catalogue
//! lives on the SYSTEM repo, not a user db.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_db::access::{principal64_from_username, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::repl_binding::{SubscriptionBinding, SUBSCRIPTION_BINDING_VERSION};
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, repl_scope, replication_profile, subscription,
};
use shamir_query_builder::doc;
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::replication::supervisor::{ReplSourceFactory, SubscriptionSupervisor};

/// Build a follower `ShamirDb` whose `app/main` destination repo uses the
/// given factory (`build_membuffer_follower` / `build_hybrid_follower`).
async fn build_follower(factory: BoxRepoFactory) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64_from_username("alice"));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg = RepoConfig::new("main", factory).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");
    Arc::new(shamir)
}

/// Follower whose `app/main` destination is MemBuffer-wrapped
/// (`Store::supports_atomic_batch() == false`).
async fn build_membuffer_follower() -> Arc<ShamirDb> {
    build_follower(BoxRepoFactory::membuffer(
        BoxRepoFactory::in_memory(),
        Default::default(),
    ))
    .await
}

/// Follower whose `app/main` destination is Hybrid: the tx-info store reports
/// `supports_atomic_batch() == true` while the repo-wide check
/// (`BoxRepo::Hybrid` arm of `validate_repo_wide_atomic_batch`) reports false
/// — exactly the mismatch the supervisor must catch before building a source.
async fn build_hybrid_follower() -> (Arc<ShamirDb>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let shamir = build_follower(BoxRepoFactory::hybrid(dir.path().join("info"))).await;
    (shamir, dir)
}

/// Create the replication profile bound to `app/main` on the follower
/// catalogue (profiles have no destination capability requirement).
async fn create_profile(follower: &ShamirDb) {
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
    follower
        .execute("app", &b.to_request_via_msgpack())
        .await
        .expect("create profile");
}

/// Run a batch op against db `app` and return the whole response or error.
async fn execute_op(
    follower: &ShamirDb,
    id: u64,
    op: shamir_query_types::batch::BatchOp,
) -> Result<shamir_query_types::batch::BatchResponse, shamir_query_types::batch::BatchError> {
    let mut batch = Batch::new();
    batch.id(id);
    batch.op("op", op);
    follower
        .execute("app", &batch.to_request_via_msgpack())
        .await
}

/// Inject a pre-existing subscription row (`state` = "active" | "paused")
/// through the REAL system-catalogue writer — the same `subscriptions_table()`
/// plus `set_via_implicit_tx` — the same writer `handle_create_subscription`
/// uses (no db name).
///
/// Round 12 F4: the row carries the SAME binding field set the
/// production CREATE path stamps (`binding_version` =
/// `SUBSCRIPTION_BINDING_VERSION`, `binding_generation` >= 1,
/// `binding_destinations` with the concrete `app`/`main` destination), so
/// `SubscriptionBinding::from_row` (the merged catalogue reader used by
/// `read_subscriptions`) parses it and reconcile reaches the backend
/// admission path these tests exist to exercise — while the destination
/// stays MemBuffer/Hybrid so the refusal still fires.
async fn inject_legacy_row(follower: &ShamirDb, name: &str, state: &str) {
    let mut row: QueryValue = doc! {
        "name" => name,
        "upstream" => "tcp://leader:9000",
        "publication" => "pub_ab",
        "profile" => "prof_ab",
        "state" => state,
    }
    .into();
    let binding = SubscriptionBinding::new(
        SUBSCRIPTION_BINDING_VERSION,
        1,
        vec![("app".to_string(), "main".to_string())],
    )
    .expect("fixture binding");
    binding.write_to_row(&mut row);
    follower
        .inject_subscription_row_for_test(row)
        .await
        .expect("inject pre-existing subscription row");
}

/// Every subscription row in the catalogue (owned maps, in storage order).
async fn all_rows(follower: &ShamirDb) -> Vec<QueryValue> {
    let table = follower
        .system_store()
        .subscriptions_table()
        .await
        .expect("subscriptions table");
    let refs = shamir_db::types::common::new_map();
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .expect("filter context");
    table
        .read(
            &shamir_db::query::read::ReadQuery::new("subscriptions"),
            &ctx,
        )
        .await
        .expect("read subscriptions")
        .records
        .into_iter()
        .map(|row| row.as_value().into_owned())
        .collect()
}

/// The row with the given `name`, if present.
async fn sub_row(follower: &ShamirDb, name: &str) -> Option<QueryValue> {
    all_rows(follower)
        .await
        .into_iter()
        .find(|row| row.get("name").and_then(|v| v.as_str()) == Some(name))
}

fn field<'a>(row: &'a QueryValue, name: &str) -> Option<&'a str> {
    row.get(name).and_then(|v| v.as_str())
}

/// R7-3 (2) / R9 (a): CREATE SUBSCRIPTION on a MemBuffer-wrapped destination
/// is refused UP FRONT with the typed `atomic_batch_unsupported` code and the
/// subscription catalogue stays EMPTY; the supervisor's reconcile of the
/// (non-existent) row neither spawns a loop nor constructs a source.
///
/// COUNTERFACTUAL: the refusal is enforced twice on this path — by the admin
/// arm of CREATE (`admin_replication.rs`) and by the seed-binding admission
/// (`bind_seed_creation` -> `control.rs` `validate_repo_wide_atomic_batch`).
/// Removing BOTH lets the CREATE persist an `active` row (first failing
/// assertion: the `err.code()` check below, then the zero-rows assertion);
/// removing only one leaves this test green through the other, which is the
/// intended defence in depth.
#[tokio::test]
async fn create_subscription_on_membuffer_destination_is_refused() {
    let follower = build_membuffer_follower().await;
    create_profile(&follower).await;

    let err = execute_op(
        &follower,
        2,
        subscription("gate_sub", "tcp://leader:9000", "pub_ab", "prof_ab"),
    )
    .await
    .expect_err("CREATE SUBSCRIPTION on a MemBuffer destination must be refused");
    assert_eq!(
        err.code(),
        Some("atomic_batch_unsupported"),
        "unexpected error: {err}"
    );

    // The catalogue is unchanged: zero rows, and the supervisor's panicking
    // factory is never reached by a reconcile of the empty catalogue.
    assert!(
        all_rows(&follower).await.is_empty(),
        "refused CREATE must leave the subscription catalogue empty"
    );
    let factory: ReplSourceFactory =
        Arc::new(|_| panic!("must refuse destination before constructing source"));
    let sup = SubscriptionSupervisor::new(follower.clone(), factory, "follower-1");
    sup.reconcile().await;
    assert!(
        !sup.is_running("gate_sub"),
        "an unsupported destination must be refused before spawning"
    );
    assert!(all_rows(&follower).await.is_empty());
    sup.stop_all().await;
}

/// R9 (b): ALTER SUBSCRIPTION ... RESUME on a MemBuffer-wrapped destination is
/// refused the same way, with the injected paused legacy row byte-identical.
///
/// COUNTERFACTUAL: the refusal is enforced twice on this path — by the RESUME
/// arm of the admin handler (`admin_replication.rs`) and by
/// `validate_seed_resume_action` -> `control.rs` `validate_seed_resume`. Removing
/// BOTH lets the resume flip the row to `active` and hand it to the
/// supervisor (first failing assertion: the `err.code()` check below, then
/// the byte-identical row comparison); removing only one leaves this test
/// green through the other, which is the intended defence in depth.
#[tokio::test]
async fn resume_on_membuffer_destination_is_refused_with_row_unchanged() {
    let follower = build_membuffer_follower().await;
    create_profile(&follower).await;
    inject_legacy_row(&follower, "gate_sub", "paused").await;
    let before = sub_row(&follower, "gate_sub").await.expect("row injected");

    let err = execute_op(
        &follower,
        3,
        alter_subscription("gate_sub")
            .resume()
            .build()
            .expect("resume op"),
    )
    .await
    .expect_err("RESUME on a MemBuffer destination must be refused");
    assert_eq!(
        err.code(),
        Some("atomic_batch_unsupported"),
        "unexpected error: {err}"
    );

    let after = sub_row(&follower, "gate_sub").await.expect("row survives");
    assert_eq!(
        before, after,
        "refused RESUME must leave the row byte-identical"
    );
}

/// R7-3 (3) / R9 (c): the SUPERVISOR refuses a Hybrid destination
/// (`BoxRepo::Hybrid` arm of `validate_repo_wide_atomic_batch` — the repo
/// reports false while its tx-info store reports true) BEFORE any source is
/// built, and persists the terminal `resync_required` status exactly once —
/// no spin, even with no events.
///
/// COUNTERFACTUAL: without the supervisor's pre-spawn check
/// (`supervisor.rs:379` `destination.validate_repo_wide_atomic_batch`) the
/// loop would spawn and the first apply would trip the receipt admission
/// refusal — warn+backoff+retry forever; the first failing assertion is
/// `calls == 0` below (the factory WOULD be constructed), followed by
/// `!sup.is_running("gate_sub")`.
#[tokio::test]
async fn hybrid_destination_is_refused_by_supervisor_without_spinning() {
    let (follower, _dir) = build_hybrid_follower().await;
    create_profile(&follower).await;
    inject_legacy_row(&follower, "gate_sub", "active").await;

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
    let row = sub_row(&follower, "gate_sub").await.expect("row persists");
    assert_eq!(field(&row, "state"), Some("resync_required"));
    let message = field(&row, "status_message").expect("terminal message");
    assert!(
        message.contains("atomic batches")
            && message.contains("raw in-memory or raw fjall repository"),
        "unexpected status_message: {message}"
    );

    // Repeated reconciles neither start a loop nor re-write the row (no spin,
    // terminal status persisted exactly once — the row is no longer `active`,
    // so reconcile skips it entirely).
    sup.reconcile().await;
    sup.reconcile().await;
    assert!(!sup.is_running("gate_sub"));
    assert_eq!(calls.load(Ordering::Acquire), 0);
    let row = sub_row(&follower, "gate_sub").await.expect("row persists");
    assert_eq!(field(&row, "state"), Some("resync_required"));
    sup.stop_all().await;
}

/// Keep the legacy MemBuffer supervisor regression: an `active` legacy row on
/// a MemBuffer destination is refused by the supervisor with the terminal
/// status and no spin (same shape as the Hybrid test above, different arm of
/// `validate_repo_wide_atomic_batch`).
#[tokio::test]
async fn membuffer_destination_is_refused_up_front() {
    let follower = build_membuffer_follower().await;
    create_profile(&follower).await;
    inject_legacy_row(&follower, "gate_sub", "active").await;
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
    let row = sub_row(&follower, "gate_sub").await.expect("row persists");
    assert_eq!(field(&row, "state"), Some("resync_required"));
    let message = field(&row, "status_message").expect("terminal message");
    assert!(
        message.contains("atomic batches")
            && message.contains("raw in-memory or raw fjall repository"),
        "unexpected status_message: {message}"
    );

    // RESUME takes the same admission path: repeated reconciles of the
    // terminal row neither spawn a loop nor flip it back to active.
    sup.reconcile().await;
    assert!(!sup.is_running("gate_sub"), "resume must be refused too");
    assert_eq!(
        field(&sub_row(&follower, "gate_sub").await.expect("row"), "state"),
        Some("resync_required"),
    );
    sup.stop_all().await;
}
