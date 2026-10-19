use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::shamir_db::shamir_db::revocation::registry::ChangelogReceiverRegistry;
use crate::shamir_db::SystemStoreConfig;
use crate::shamir_db::{ChangelogDeliveryGate, ChangelogReceiver};
use crate::ShamirDb;

#[tokio::test]
async fn close_discards_queued_feed_events_and_leaves_healthy_receiver() {
    let (tx, rx) = tokio::sync::broadcast::channel(4);
    let registry = ChangelogReceiverRegistry::new();
    let mut revoked = ChangelogReceiver::new(rx, registry.clone(), "testdb".into(), "main".into());
    let gate = revoked.delivery_gate();
    let mut healthy = ChangelogReceiver::new(
        tx.subscribe(),
        registry.clone(),
        "testdb".into(),
        "side".into(),
    );
    registry.register("testdb", "main", revoked.delivery_gate());
    registry.register("testdb", "side", healthy.delivery_gate());
    let event = std::sync::Arc::new(shamir_engine::ChangelogEvent {
        repo: "main".into(),
        commit_version: 1,
        tx_id: 1,
        actor: crate::access::Actor::System,
        timestamp_ns: 0,
        changes: Vec::new(),
    });
    tx.send(event.clone()).unwrap();
    tx.send(event).unwrap();
    gate.close();
    gate.wait_idle().await;
    assert!(matches!(
        revoked.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
    assert!(healthy.try_recv().is_ok());
    assert!(!healthy.delivery_gate().closed_now());
    drop(revoked);
    assert!(!registry
        .snapshot()
        .iter()
        .any(|((_, repo), _)| repo == "main"));
}

#[tokio::test]
async fn persisted_quarantine_flip_revokes_idle_receiver_without_post_flip_write() {
    let db = ShamirDb::init(SystemStoreConfig::InMemory).await.unwrap();
    let instance = db.create_db("testdb").await;
    instance
        .add_repo(
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("users")),
        )
        .await
        .unwrap();
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id(1);
    batch.op(
        "profile",
        shamir_query_builder::ddl::replication_profile("prof").stream(
            shamir_query_builder::ddl::repl_scope("testdb")
                .repo("main")
                .build(),
            shamir_query_types::admin::ReplDirection::Pull,
            shamir_query_types::admin::ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        shamir_query_builder::ddl::subscription("sub", "tcp://peer:9000", "pub", "prof"),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    let mut receiver = db
        .subscribe_changelog("testdb", "main")
        .await
        .unwrap()
        .unwrap();
    let gate = receiver.delivery_gate();
    db.mark_subscription_resync_required("sub", 7, 3)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), gate.closed())
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    ));
}

#[tokio::test]
async fn wait_idle_stays_pending_until_active_delivery_lease_releases() {
    let gate = ChangelogDeliveryGate::new();
    let worker_gate = gate.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        worker_gate.try_with_delivery(|| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });

    entered_rx.await.unwrap();
    gate.close();
    let mut wait_idle = Box::pin(gate.wait_idle());
    let returned_while_active = tokio::select! {
        biased;
        _ = &mut wait_idle => true,
        _ = std::future::ready(()) => false,
    };
    release_tx.send(()).unwrap();
    if !returned_while_active {
        wait_idle.await;
    }
    assert_eq!(worker.join().unwrap(), Some(()));
    assert!(
        !returned_while_active,
        "wait_idle completed while a delivery lease was active"
    );
}

#[tokio::test]
async fn delivery_gate_is_panic_safe_when_closed() {
    let gate = ChangelogDeliveryGate::new();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = gate.try_with_delivery(|| panic!("lease panic"));
    }));
    assert!(panic.is_err());
    gate.close();
    gate.wait_idle().await;
    assert!(gate.closed_now());
    assert!(gate.try_with_delivery(|| ()).is_none());
}

/// Round 6 item 5 ORACLE: a pending key leaves the cache ONLY through an
/// explicit `confirm_pending` / `rollback_pending` — `install` (a refresh)
/// is NOT a persistence acknowledgement and must keep unmatched pending
/// keys (entries included) so reads/guards never regress below the
/// prepublished set.
/// COUNTERFACTUAL: reverting `install` to dropping unmatched pending keys
/// (pre-round-6) turns the two post-refresh `pending` assertions red.
#[tokio::test]
async fn quarantine_cache_replace_clears_persisted_removal_but_keeps_pending_failure() {
    let cache = crate::shamir_db::execute::ResyncQuarantineCache::new();
    let active = ("db".to_string(), "active".to_string());
    let pending = ("db".to_string(), "pending".to_string());
    cache.publish([(pending.clone(), "prepublished".to_string())]);
    // A refresh that derives other rows must keep the pending key protected.
    cache.install([(active.clone(), "persisted".to_string())]);
    assert!(cache.reason_for("db", "active").is_some());
    assert!(cache.reason_for("db", "pending").is_some());
    cache.install([(pending.clone(), "persisted pending".to_string())]);
    assert!(cache.reason_for("db", "pending").is_some());
    // Refresh is NOT an acknowledgement: even an install deriving NOTHING
    // keeps the pending key (and its reason) until explicit confirmation.
    cache.install(Vec::<((String, String), String)>::new());
    assert!(cache.reason_for("db", "pending").is_some());
    // confirm_pending drops the entry (no refresh derived it anymore).
    cache.confirm_pending([pending.clone()]);
    assert!(cache.reason_for("db", "pending").is_none());
    // rollback_pending is the definitive variant: pending key gone, entry
    // dropped, whatever the refresh derived.
    cache.publish([(pending.clone(), "failed prepublish".to_string())]);
    cache.install(Vec::<((String, String), String)>::new());
    assert!(cache.reason_for("db", "pending").is_some());
    cache.rollback_pending([pending.clone()]);
    assert!(cache.reason_for("db", "pending").is_none());
}

#[tokio::test]
async fn marking_after_profile_removal_quarantines_exactly_bound_destination() {
    let db = ShamirDb::init(SystemStoreConfig::InMemory).await.unwrap();
    let instance = db.create_db("testdb").await;
    instance
        .add_repo(
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("users")),
        )
        .await
        .unwrap();
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id(1);
    batch.op(
        "profile",
        shamir_query_builder::ddl::replication_profile("prof").stream(
            shamir_query_builder::ddl::repl_scope("testdb")
                .repo("main")
                .build(),
            shamir_query_types::admin::ReplDirection::Pull,
            shamir_query_types::admin::ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "sub",
        shamir_query_builder::ddl::subscription("sub", "tcp://peer:9000", "pub", "prof"),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    // Remove the profile: S5c-X PR8b-2 — coverage comes from the row's
    // durable binding, so profile resolution is never consulted and marking
    // must still succeed.
    let mut drop_batch = shamir_query_builder::batch::Batch::new();
    drop_batch.id(2);
    drop_batch.op(
        "profile",
        shamir_query_builder::ddl::drop_replication_profile("prof"),
    );
    db.execute("testdb", &drop_batch.to_request_via_msgpack())
        .await
        .unwrap();

    db.mark_subscription_resync_required("sub", 7, 3)
        .await
        .unwrap();

    // The bound destination is quarantined with the row's status reason.
    let reason = db
        .read_quarantine_reason("testdb", "main")
        .expect("bound destination must be quarantined");
    assert!(
        reason.contains("journal gap at 7"),
        "reason must come from the marking message, got: {reason}"
    );
    // And nothing outside the bound destination set is quarantined — no
    // global sentinel exists anymore.
    assert!(db.read_quarantine_reason("otherdb", "other").is_none());
}
