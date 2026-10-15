//! S5c-X PR8b — the journal-drain source a subscription bridge reads through
//! (`DbJournalSource::read` → `ShamirDb::read_changelog_bounded`) fails
//! closed once the repo's downstream subscription is flipped to
//! `resync_required`: `JournalDrain::run` propagates that `Err`
//! (`src/subscriptions/journal_drain.rs`), and `bridge_task` logs "journal
//! subscription terminated" and exits (`src/subscriptions/bridge.rs`).
//! `DbJournalSource` itself is `pub(super)`, so this test proves the
//! mechanism at the exact public call the drain makes:
//! `ShamirDb::read_changelog_bounded` must be `Ok` before the quarantine
//! flip and `Err` (`resync_required`) after it. Fixture mirrors shamir-db's
//! `quarantine_reads_tests.rs` (profile + subscription batch, then
//! `mark_subscription_resync_required` — the real persisted path).
//!
//! The second test attaches a REAL bridge subscription BEFORE the quarantine
//! and asserts the bridge terminates (`PushKind::Closed`) without ever
//! delivering the post-quarantine row.

use std::sync::Arc;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::{PushRejected, PushSink};
use shamir_db::access::{principal64, Actor, ResourceMeta, ResourcePath};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::subscribe::SourceBuilder;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::BatchLimits;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use tokio::sync::mpsc;

use super::super::super::bridge::bridge_task;
use super::super::super::registry::SubscriptionRegistry;

/// In-memory `ShamirDb` with db "app" and repo "main" (one "items" table) —
/// same fixture style as `bridge_snapshot_cut_tests.rs::database`.
async fn database() -> ShamirDb {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    db.add_repo_as("app", cfg, owner).await.expect("add repo");
    db
}

/// Create a Pull profile + ACTIVE subscription covering `app/main`, then
/// flip it to `resync_required` through the real persisted path.
///
/// The batch runs as `Actor::System`: creating a replication profile is a
/// root-only `Manage` action on System-owned meta (`0o751`), which a plain
/// user actor is not permitted.
async fn quarantine(db: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "profile",
        replication_profile("prof").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription("sub", "tcp://peer:9000", "pub", "prof"));
    db.execute_as(Actor::System, "app", &b.build())
        .await
        .expect("admin replication batch must succeed");
    db.mark_subscription_resync_required("sub", 7, 3)
        .await
        .expect("mark resync_required must succeed");
}

/// The journal drain's read source is `Ok` while the repo is healthy and
/// fails closed with `resync_required` the moment the subscription is
/// quarantined — so a bridge attached BEFORE the quarantine terminates on
/// its next drain read.
#[tokio::test]
async fn journal_drain_read_fails_closed_after_quarantine_flip() {
    let db = database().await;

    // Healthy: the drain source read succeeds (journal attached, may be
    // `Some` with zero events on a fresh repo).
    let page = db
        .read_changelog_bounded("app", "main", 1, 10)
        .await
        .expect("journal read must succeed while healthy");
    assert!(page.is_some(), "fresh repo journal must be readable");

    quarantine(&db).await;

    let Err(err) = db.read_changelog_bounded("app", "main", 1, 10).await else {
        panic!("journal read must be refused once quarantined");
    };
    assert_eq!(
        err.code(),
        Some("resync_required"),
        "drain source must fail closed with resync_required"
    );
}

// ---------------------------------------------------------------------------
// Live-bridge termination test — fixture helpers copied verbatim from
// `bridge_snapshot_cut_tests.rs` (its `Sink`/`PushSink`, row decoder and
// resource-meta opening are module-private there).
// ---------------------------------------------------------------------------

struct Sink(mpsc::Sender<PushEnvelope>);

impl PushSink for Sink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        self.0
            .try_send(rmp_serde::from_slice(&frame).unwrap())
            .map_err(|_| PushRejected)
    }
}

#[derive(Debug, serde::Deserialize)]
struct Row {
    #[allow(dead_code)]
    table: String,
    #[allow(dead_code)]
    commit_version: u64,
    value: Option<QueryValue>,
}

fn row_id(frame: &PushEnvelope) -> String {
    assert_eq!(frame.push, PushKind::Event);
    let decoded: Row = rmp_serde::from_slice(frame.data.as_deref().unwrap()).unwrap();
    let value = decoded.value.expect("Records delivery includes the row");
    let Some(QueryValue::Str(key)) = value.get("_id") else {
        panic!("row must contain its string _id");
    };
    key
}

/// Same shape as `bridge_snapshot_cut_tests.rs::database`, but for the
/// `app`/`main`/`users` triple this file's ACL fixture uses; every ancestor
/// and the source table get `ResourceMeta::open()` exactly like the
/// snapshot-cut fixture does, so the bridge's per-table read-ACL checks pass.
async fn bridge_database() -> Arc<ShamirDb> {
    let db = Arc::new(ShamirDb::init_memory().await.expect("init shamir"));
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo_as("app", cfg, owner).await.expect("add repo");
    // Fresh catalogues are creator-private (0o700), not ResourceMeta::default().
    // Open the ancestors and source tables explicitly (as the snapshot-cut
    // fixture does).
    for path in [
        ResourcePath::database("app"),
        ResourcePath::store("app", "main"),
        ResourcePath::table("app", "main", "users"),
    ] {
        db.set_resource_meta(&path, &ResourceMeta::open())
            .await
            .unwrap();
    }
    db
}

async fn write_user(db: &Arc<ShamirDb>, key: &str) {
    let mut b = Batch::new();
    b.id(key.len() as u64);
    b.insert("u", insert("users").row(doc! { "_id" => key }));
    db.execute_as(Actor::System, "app", &b.build())
        .await
        .unwrap();
}

/// A bridge attached BEFORE the quarantine receives its snapshot + live
/// event, then terminates with `PushKind::Closed` once the repo is
/// quarantined, and never delivers the post-quarantine row (its delivery
/// would require a journal page read, which the gate refuses).
#[tokio::test]
async fn live_bridge_terminates_on_quarantine_and_stops_delivery() {
    let db = bridge_database().await;
    write_user(&db, "seed").await;

    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(1);
    let (frames, mut observed) = mpsc::channel(32);
    let bridge = tokio::spawn(bridge_task(
        7,
        db.clone(),
        "app".to_owned(),
        vec![SourceBuilder::table("users").build()],
        DeliverMode::Records,
        Actor::System,
        Arc::new(Sink(frames)),
        None,
        true, // initial: run the snapshot
        BatchLimits::default(),
        registry.clone(),
    ));

    // Post-subscribe live write.
    write_user(&db, "live").await;

    // Snapshot: the seed row event, then the Ready frame.
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), observed.recv())
        .await
        .expect("snapshot event must arrive")
        .expect("sender alive");
    assert_eq!(first.push, PushKind::Event);
    assert_eq!(row_id(&first), "seed");
    let ready = tokio::time::timeout(std::time::Duration::from_secs(5), observed.recv())
        .await
        .expect("Ready must arrive")
        .expect("sender alive");
    assert_eq!(ready.push, PushKind::Ready);

    // Exactly ONE live event (the "live" row) — the bridge is now attached.
    let live = tokio::time::timeout(std::time::Duration::from_secs(5), observed.recv())
        .await
        .expect("live event must arrive")
        .expect("sender alive");
    assert_eq!(live.push, PushKind::Event);
    assert_eq!(row_id(&live), "live");

    // Quarantine through the real persisted path (profile + subscription
    // creation batch as Actor::System, then mark resync_required).
    quarantine(&db).await;

    // Second row AFTER the quarantine: the bridge must terminate with a
    // `Closed` frame and never deliver this row.
    write_user(&db, "post").await;

    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), observed.recv())
        .await
        .expect("bridge must emit Closed after quarantine")
        .expect("sender alive");
    assert_eq!(
        closed.push,
        PushKind::Closed,
        "first frame after the quarantine write must be Closed"
    );

    // Drain whatever remains (channel is bounded and the bridge is dead once
    // Closed was pushed) — nothing may carry the post-quarantine row.
    while let Ok(frame) = observed.try_recv() {
        assert_ne!(
            frame.push,
            PushKind::Event,
            "no event may follow the quarantine"
        );
    }

    // The bridge task must have exited cleanly (its registry slot released).
    tokio::time::timeout(std::time::Duration::from_secs(5), bridge)
        .await
        .expect("bridge must exit after quarantine")
        .expect("bridge task must not panic");
    assert_eq!(registry.count(), 0, "bridge slot must be released");
}
