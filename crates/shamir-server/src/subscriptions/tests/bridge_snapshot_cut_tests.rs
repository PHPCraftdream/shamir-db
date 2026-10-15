//! Exact snapshot/journal cut regressions; all interleavings use an explicit hook.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::FutureExt;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::{PushRejected, PushSink};
use shamir_db::access::{Actor, ResourceMeta, ResourcePath};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::subscribe::SourceBuilder;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::BatchLimits;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use tokio::sync::{mpsc, Notify};

use super::super::bridge::bridge_task;
use super::super::registry::SubscriptionRegistry;

pub(crate) struct Hook {
    pub(crate) stage: u8,
    pub(crate) cut: AtomicU64,
    pub(crate) entered: Notify,
    pub(crate) release: Notify,
}

impl Hook {
    pub(crate) fn new(stage: u8) -> Self {
        Self {
            stage,
            cut: AtomicU64::new(0),
            entered: Notify::new(),
            release: Notify::new(),
        }
    }
}

tokio::task_local! {
    pub(crate) static HOOK: Arc<Hook>;
}

// Task-local, so unrelated bridges and parallel tests cannot consume the hook.
// cancel-safe: yes — terminal cancellation abandons only test observation.
pub(crate) async fn pause(stage: u8, cut: u64) {
    let hook = HOOK.try_with(Arc::clone).ok();
    if let Some(hook) = hook {
        if hook.stage == stage {
            hook.cut.store(cut, Ordering::Release);
            hook.entered.notify_one();
            hook.release.notified().await;
        }
    }
}

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
    table: String,
    commit_version: u64,
    value: Option<QueryValue>,
}

fn row(frame: &PushEnvelope) -> Row {
    assert_eq!(frame.push, PushKind::Event);
    rmp_serde::from_slice(frame.data.as_deref().unwrap()).unwrap()
}

async fn database() -> Arc<ShamirDb> {
    let db = Arc::new(ShamirDb::init_memory().await.unwrap());
    db.create_db("test_db").await;
    db.add_repo(
        "test_db",
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"))
            .add_table(TableConfig::new("other")),
    )
    .await
    .unwrap();
    // Fresh catalogues are creator-private (0o700), not ResourceMeta::default().
    // Open the ancestors and source tables explicitly for the user-actor tests.
    for path in [
        ResourcePath::database("test_db"),
        ResourcePath::store("test_db", "main"),
        ResourcePath::table("test_db", "main", "users"),
        ResourcePath::table("test_db", "main", "other"),
    ] {
        db.set_resource_meta(&path, &ResourceMeta::open())
            .await
            .unwrap();
    }
    db
}

async fn write(db: Arc<ShamirDb>, key: &str, both: bool) {
    let mut batch = Batch::new();
    batch.transactional();
    batch.insert(
        "user",
        insert("users").row(doc! {
            "_id" => key,
            "group" => 7
        }),
    );
    if both {
        batch.insert(
            "other",
            insert("other").row(doc! {
                "_id" => key
            }),
        );
    }
    db.execute_as(Actor::System, "test_db", &batch.build())
        .await
        .unwrap();
}

// stage 0 commits after activation W but before S: old code replays the row.
// stage 1 commits after S but before any read: old code snapshots AND replays it.
// stage 2 commits between targets: old code sees the new row in only target two.
async fn interleaving(stage: u8, resume: bool) {
    let db = database().await;
    write(db.clone(), "seed", true).await;
    let old = db.changelog_visible_floor("test_db", "main").await.unwrap();
    let gate = db
        .get_db("test_db")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(1);
    let (frames, mut observed) = mpsc::channel(32);
    let hook = Arc::new(Hook {
        stage,
        cut: AtomicU64::new(0),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut bridge = Box::pin(HOOK.scope(
        hook.clone(),
        bridge_task(
            1,
            db.clone(),
            "test_db".to_owned(),
            vec![
                SourceBuilder::table("users").build(),
                SourceBuilder::table("other").build(),
            ],
            DeliverMode::Records,
            Actor::System,
            Arc::new(Sink(frames)),
            resume.then_some(old),
            true,
            BatchLimits::default(),
            registry.clone(),
        ),
    ));
    tokio::select! {
        () = &mut bridge => panic!("bridge exited before hook"),
        () = hook.entered.notified() => {}
    }
    if stage != 0 {
        assert!(
            !gate.active_snapshots_empty(),
            "snapshot must remain pinned"
        );
        assert_eq!(hook.cut.load(Ordering::Acquire), old);
    }
    // Spawn rather than awaiting a committer inline while a lower version may park.
    let writer_db = db.clone();
    let writer = tokio::spawn(async move { write(writer_db, "during", true).await });
    writer.await.unwrap();
    let during = db.changelog_visible_floor("test_db", "main").await.unwrap();
    assert!(during > old);
    hook.release.notify_one();
    let mut all = Vec::new();
    loop {
        let frame = tokio::select! {
            biased;
            frame = observed.recv() => frame.unwrap(),
            () = &mut bridge => panic!("bridge exited before Ready"),
        };
        let ready = frame.push == PushKind::Ready;
        all.push(frame);
        if ready {
            break;
        }
    }
    assert!(
        gate.active_snapshots_empty(),
        "successful snapshot releases pin",
    );
    if stage != 0 {
        for _ in 0..2 {
            all.push(tokio::select! {
                biased;
                frame = observed.recv() => frame.unwrap(),
                () = &mut bridge => panic!("bridge exited before live events"),
            });
        }
    }
    // An observable later transaction fences off any replay of <= S.
    write(db.clone(), "fence", false).await;
    all.push(tokio::select! {
        biased;
        frame = observed.recv() => frame.unwrap(),
        () = &mut bridge => panic!("bridge exited before fence"),
    });
    let versions: Vec<_> = all
        .iter()
        .filter(|f| f.push == PushKind::Event)
        .map(|f| row(f).commit_version)
        .collect();
    let mut expected = if resume { vec![old, old] } else { Vec::new() };
    expected.extend(if stage == 0 {
        vec![0, 0, 0, 0]
    } else {
        vec![0, 0, during, during]
    });
    let fence = db.changelog_visible_floor("test_db", "main").await.unwrap();
    expected.push(fence);
    assert_eq!(versions, expected);
    assert_eq!(
        all.iter().position(|f| f.push == PushKind::Ready),
        Some(if resume || stage == 0 { 4 } else { 2 }),
    );
    let tables: Vec<_> = all
        .iter()
        .filter(|f| f.push == PushKind::Event)
        .map(|f| row(f).table)
        .collect();
    assert_eq!(
        tables.iter().filter(|t| *t == "other").count(),
        if resume { 3 } else { 2 },
    );
    // Check row identities as well as counts/versions: a repeated seed cannot
    // stand in for the concurrent row, and neither target can read past S.
    let mut identities: Vec<_> = all
        .iter()
        .filter(|frame| frame.push == PushKind::Event)
        .map(|frame| {
            let payload = row(frame);
            let value = payload.value.expect("Records delivery includes the row");
            let Some(QueryValue::Str(key)) = value.get("_id") else {
                panic!("row must contain its string _id");
            };
            (payload.table, key.clone(), payload.commit_version)
        })
        .collect();
    let mut expected_identities = Vec::new();
    for table in ["users", "other"] {
        if resume {
            expected_identities.push((table.to_owned(), "seed".to_owned(), old));
        }
        expected_identities.push((table.to_owned(), "seed".to_owned(), 0));
        expected_identities.push((
            table.to_owned(),
            "during".to_owned(),
            if stage == 0 { 0 } else { during },
        ));
    }
    expected_identities.push(("users".to_owned(), "fence".to_owned(), fence));
    identities.sort();
    expected_identities.sort();
    assert_eq!(identities, expected_identities);
    // Poll once to drain already available work; no wall-clock waiting.
    assert!((&mut bridge).now_or_never().is_none());
    assert!(observed.try_recv().is_err(), "no duplicate suffix");
    drop(bridge);
    assert_eq!(registry.count(), 0);
    assert!(gate.active_snapshots_empty());
}

#[tokio::test]
async fn committed_through_s_is_snapshot_only() {
    interleaving(0, false).await;
}

#[tokio::test]
async fn committed_after_s_is_journal_only() {
    interleaving(1, false).await;
}

#[tokio::test]
async fn two_targets_share_s() {
    interleaving(2, false).await;
}

#[tokio::test]
async fn resume_precedes_snapshot_ready_and_live() {
    interleaving(1, true).await;
}

// Old code logs a failed query (or ignores configured limits) and emits Ready.
#[tokio::test]
async fn snapshot_failure_closes_without_ready_and_releases_pin() {
    for missing in [false, true] {
        let db = database().await;
        let gate = db
            .get_db("test_db")
            .unwrap()
            .get_repo("main")
            .unwrap()
            .tx_gate()
            .await
            .unwrap();
        let registry = Arc::new(SubscriptionRegistry::new());
        registry.try_reserve().unwrap();
        registry.reserve_pending(1);
        let (frames, mut observed) = mpsc::channel(8);
        let limits = if missing {
            BatchLimits::default()
        } else {
            BatchLimits {
                max_queries: 0,
                ..BatchLimits::default()
            }
        };
        let mut bridge = Box::pin(bridge_task(
            1,
            db,
            "test_db".to_owned(),
            vec![SourceBuilder::table(if missing { "missing" } else { "users" }).build()],
            DeliverMode::Records,
            Actor::System,
            Arc::new(Sink(frames)),
            None,
            true,
            limits,
            registry.clone(),
        ));
        let terminal = tokio::select! {
            biased;
            frame = observed.recv() => frame.unwrap(),
            () = &mut bridge => observed.try_recv().unwrap(),
        };
        assert_eq!(terminal.push, PushKind::Closed);
        // The Closed enqueue happens in the bridge's final synchronous poll.
        drop(bridge);
        assert!(observed.try_recv().is_err());
        assert!(gate.active_snapshots_empty());
        assert_eq!(registry.count(), 0);
    }
}

// Compatibility control: snapshot filters and denied-source exclusion still apply.
#[tokio::test]
async fn snapshot_preserves_actor_filters_and_authorization() {
    let db = database().await;
    write(db.clone(), "allowed", true).await;
    let mut batch = Batch::new();
    batch.insert(
        "row",
        insert("users").row(doc! {
            "_id" => "filtered",
            "group" => 8
        }),
    );
    db.execute_as(Actor::System, "test_db", &batch.build())
        .await
        .unwrap();
    db.set_resource_meta(
        &ResourcePath::table("test_db", "main", "other"),
        &ResourceMeta {
            owner: Actor::User(999),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(1);
    let (frames, mut observed) = mpsc::channel(8);
    let mut bridge = Box::pin(bridge_task(
        1,
        db,
        "test_db".to_owned(),
        vec![
            SourceBuilder::table("users").filter(eq("group", 7)).build(),
            SourceBuilder::table("other").build(),
        ],
        DeliverMode::Records,
        Actor::User(42),
        Arc::new(Sink(frames)),
        None,
        true,
        BatchLimits::default(),
        registry.clone(),
    ));
    let snapshot = tokio::select! {
        biased;
        frame = observed.recv() => frame.unwrap(),
        () = &mut bridge => panic!("authorized snapshot terminated"),
    };
    let payload = row(&snapshot);
    assert_eq!(payload.table, "users");
    assert_eq!(payload.commit_version, 0);
    assert_eq!(
        payload.value.unwrap().get("_id"),
        Some(&QueryValue::Str("allowed".into())),
    );
    let ready = tokio::select! {
        biased;
        frame = observed.recv() => frame.unwrap(),
        () = &mut bridge => panic!("authorized snapshot terminated"),
    };
    assert_eq!(ready.push, PushKind::Ready);
    assert_eq!(ready.seq, 1);
    drop(bridge);
    assert_eq!(registry.count(), 0);
}

// Negative control for actor propagation: activation succeeds, then access is
// revoked after begin. Executing snapshot reads as System would emit Ready.
#[tokio::test]
async fn snapshot_reauthorizes_reads_as_subscriber() {
    let db = database().await;
    write(db.clone(), "private", false).await;
    let gate = db
        .get_db("test_db")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(1);
    let (frames, mut observed) = mpsc::channel(8);
    let hook = Arc::new(Hook {
        stage: 1,
        cut: AtomicU64::new(0),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut bridge = Box::pin(HOOK.scope(
        hook.clone(),
        bridge_task(
            1,
            db.clone(),
            "test_db".to_owned(),
            vec![SourceBuilder::table("users").build()],
            DeliverMode::Records,
            Actor::User(42),
            Arc::new(Sink(frames)),
            None,
            true,
            BatchLimits::default(),
            registry.clone(),
        ),
    ));
    tokio::select! {
        () = &mut bridge => panic!("bridge exited before snapshot begin"),
        () = hook.entered.notified() => {}
    }
    db.set_resource_meta(
        &ResourcePath::table("test_db", "main", "users"),
        &ResourceMeta {
            owner: Actor::User(999),
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    hook.release.notify_one();
    let terminal = tokio::select! {
        biased;
        frame = observed.recv() => frame.unwrap(),
        () = &mut bridge => observed.try_recv().unwrap(),
    };
    assert_eq!(terminal.push, PushKind::Closed);
    drop(bridge);
    assert!(gate.active_snapshots_empty());
    assert_eq!(registry.count(), 0);
    assert!(observed.try_recv().is_err());
}

#[tokio::test]
async fn cancellation_during_snapshot_releases_pin_and_registry() {
    let db = database().await;
    let gate = db
        .get_db("test_db")
        .unwrap()
        .get_repo("main")
        .unwrap()
        .tx_gate()
        .await
        .unwrap();
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(1);
    let (frames, mut observed) = mpsc::channel(8);
    let hook = Arc::new(Hook {
        stage: 1,
        cut: AtomicU64::new(0),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut bridge = Box::pin(HOOK.scope(
        hook.clone(),
        bridge_task(
            1,
            db,
            "test_db".to_owned(),
            vec![SourceBuilder::table("users").build()],
            DeliverMode::Records,
            Actor::System,
            Arc::new(Sink(frames)),
            None,
            true,
            BatchLimits::default(),
            registry.clone(),
        ),
    ));
    tokio::select! {
        () = &mut bridge => panic!("bridge exited before snapshot begin"),
        () = hook.entered.notified() => {}
    }
    assert!(!gate.active_snapshots_empty());
    drop(bridge);
    assert!(gate.active_snapshots_empty());
    assert_eq!(registry.count(), 0);
    assert!(observed.try_recv().is_err());
}
