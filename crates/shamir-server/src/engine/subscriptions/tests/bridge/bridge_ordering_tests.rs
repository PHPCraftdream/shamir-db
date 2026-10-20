use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::{PushRejected, PushSink};
use shamir_db::access::Actor;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::BoundedJournalRead;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::subscribe::SourceBuilder;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::BatchLimits;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use shamir_tx::{ChangelogEvent, JournalRead};
use tokio::sync::{broadcast, mpsc, Notify};

use crate::subscriptions::bridge::bridge_task;
use crate::subscriptions::journal_drain::JournalDrain;
use crate::subscriptions::journal_source::JournalSource;
use crate::subscriptions::push::try_push_event;
use crate::subscriptions::registry::SubscriptionRegistry;

// Controllable source, not an ordering implementation: each response and expected
// range start is supplied by the test. Engine hooks are cfg(test)/pub(crate) in
// shamir-engine and cannot be reached from this server crate's test build.
struct Source {
    pages: Mutex<VecDeque<(u64, Result<BoundedJournalRead, String>)>>,
    reads: AtomicUsize,
    waits: AtomicUsize,
    entered: Notify,
    idle: Notify,
    release: Notify,
    park_progress: bool,
}

impl Source {
    fn new(pages: Vec<(u64, BoundedJournalRead)>) -> Self {
        Self {
            pages: Mutex::new(
                pages
                    .into_iter()
                    .map(|(from, page)| (from, Ok(page)))
                    .collect(),
            ),
            reads: AtomicUsize::new(0),
            waits: AtomicUsize::new(0),
            entered: Notify::new(),
            idle: Notify::new(),
            release: Notify::new(),
            park_progress: false,
        }
    }
}

impl JournalSource for Source {
    fn on_idle(&self) {
        self.idle.notify_one();
    }

    async fn read(&self, _repo: &str, from: u64) -> Result<BoundedJournalRead, String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        // Test-only scripting lock; dropped before any suspension.
        let (expected, page) = self
            .pages
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected read");
        assert_eq!(
            from, expected,
            "cursor must reflect completely accepted history",
        );
        page
    }

    /// cancel-safe: yes — single-consumer notifies; dropping only abandons observation.
    async fn progress(&self, _repo: &str, target: u64) -> Result<(), String> {
        assert_eq!(target, 11);
        self.waits.fetch_add(1, Ordering::Relaxed);
        self.entered.notify_one();
        if self.park_progress {
            self.release.notified().await;
        }
        Ok(())
    }
}

fn event(version: u64) -> ChangelogEvent {
    ChangelogEvent {
        repo: "main".to_owned(),
        commit_version: version,
        tx_id: version,
        actor: Actor::System,
        timestamp_ns: 0,
        changes: Vec::new(),
    }
}

fn page(versions: &[u64], bound: u64, w: u64, confirmed: bool) -> BoundedJournalRead {
    BoundedJournalRead {
        read: JournalRead {
            events: versions.iter().copied().map(event).collect(),
            gap_at: None,
        },
        sampled_w: w,
        bound,
        confirmed,
    }
}

// Red with broadcast-order delivery: B is sent first and A disappears.
#[tokio::test]
async fn reversed_hints_and_lag_recover_exact_journal_set() {
    for capacity in [1, 16] {
        let source = Source::new(vec![
            (10, page(&[10, 11], 11, 11, true)),
            (12, page(&[], 11, 11, true)),
        ]);
        let (tx, rx) = broadcast::channel(capacity);
        tx.send(Arc::new(event(11))).unwrap();
        tx.send(Arc::new(event(10))).unwrap();
        let repos = vec!["main".to_owned()];
        let mut states = vec![JournalDrain::new(9)];
        let (out, mut observed) = mpsc::channel(16);
        JournalDrain::run(
            &source,
            &repos,
            &mut [rx],
            &mut states,
            true,
            |_, event| {
                out.try_send(event.commit_version).unwrap();
                std::future::ready(true)
            },
            |_| async { panic!("lag is not a journal gap") },
        )
        .await
        .unwrap();
        assert_eq!(observed.try_recv().unwrap(), 10);
        assert_eq!(observed.try_recv().unwrap(), 11);
        assert!(observed.try_recv().is_err());
        assert_eq!(states[0].cursor, 11);
    }
}

// Red with one-shot backfill or short-page exhaustion, and with frame-only cursors.
#[tokio::test]
async fn short_filtered_sparse_pages_continue_until_confirmed_empty() {
    let source = Source::new(vec![
        (2, page(&[2], 20, 20, true)),
        (3, page(&[7], 20, 20, true)),
        (8, page(&[19], 20, 20, true)),
        (20, page(&[], 20, 20, true)),
    ]);
    let (_tx, rx) = broadcast::channel(4);
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(1)];
    let (out, mut observed) = mpsc::channel(4);
    JournalDrain::run(
        &source,
        &repos,
        &mut [rx],
        &mut states,
        true,
        |_, event| {
            if event.commit_version == 19 {
                out.try_send(19).unwrap();
            }
            std::future::ready(true)
        },
        |_| async { panic!("unexpected gap") },
    )
    .await
    .unwrap();
    assert_eq!(observed.try_recv().unwrap(), 19);
    assert!(observed.try_recv().is_err());
    assert_eq!(states[0].cursor, 20);
    assert_eq!(source.reads.load(Ordering::Relaxed), 4);
}

// Red when the final hint is consumed and timeout returns to hint-only waiting.
#[tokio::test(start_paused = true)]
async fn confirmation_timeout_retries_without_another_commit() {
    let source = Source::new(vec![
        (10, page(&[10], 10, 11, false)),
        (11, page(&[], 10, 11, false)),
        (11, page(&[11], 11, 11, true)),
        (12, page(&[], 11, 11, true)),
    ]);
    let (_tx, rx) = broadcast::channel(4);
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(9)];
    let (out, mut observed) = mpsc::channel(4);
    JournalDrain::run(
        &source,
        &repos,
        &mut [rx],
        &mut states,
        true,
        |_, event| {
            out.try_send(event.commit_version).unwrap();
            std::future::ready(true)
        },
        |_| async { panic!("unexpected gap") },
    )
    .await
    .unwrap();
    assert_eq!(observed.try_recv().unwrap(), 10);
    assert_eq!(observed.try_recv().unwrap(), 11);
    assert!(observed.try_recv().is_err());
    assert_eq!(states[0].cursor, 11);
}

// Red when unresolved A's abort (no payload hint) cannot release B's delivery.
#[tokio::test]
async fn unresolved_prefix_parks_then_abort_progress_releases_without_hint() {
    let mut source = Source::new(vec![
        (10, page(&[], 9, 9, true)),
        (10, page(&[], 9, 9, true)),
        (10, page(&[11], 11, 11, true)),
        (12, page(&[], 11, 11, true)),
    ]);
    source.park_progress = true;
    let (tx, rx) = broadcast::channel(4);
    tx.send(Arc::new(event(11))).unwrap();
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(9)];
    let (out, mut observed) = mpsc::channel(4);
    let mut receivers = [rx];
    let drain = JournalDrain::run(
        &source,
        &repos,
        &mut receivers,
        &mut states,
        true,
        |_, event| {
            out.try_send(event.commit_version).unwrap();
            std::future::ready(true)
        },
        |_| async { panic!("unresolved prefix is not a gap") },
    );
    tokio::pin!(drain);
    tokio::select! {
        result = &mut drain => panic!("premature completion: {result:?}"),
        () = source.entered.notified() => {}
    }
    assert!(observed.try_recv().is_err());
    source.release.notify_one();
    drain.await.unwrap();
    assert_eq!(observed.try_recv().unwrap(), 11);
    assert!(observed.try_recv().is_err());
    assert_eq!(source.waits.load(Ordering::Relaxed), 1);
}

// Red when gap permits suffix delivery, or when errors become empty success.
#[tokio::test]
async fn journal_gap_and_read_error_are_terminal_without_suffix() {
    for error in [false, true] {
        let mut first = page(&[11], 11, 11, true);
        first.read.gap_at = Some(10);
        let source = Source::new(vec![(10, first)]);
        if error {
            source.pages.lock().unwrap()[0].1 = Err("failed range".to_owned());
        }
        let (_tx, rx) = broadcast::channel(4);
        let repos = vec!["main".to_owned()];
        let mut states = vec![JournalDrain::new(9)];
        let (out, mut observed) = mpsc::channel(4);
        let result = JournalDrain::run(
            &source,
            &repos,
            &mut [rx],
            &mut states,
            true,
            |_, _| async { panic!("terminal page cannot deliver suffix") },
            |at| {
                out.try_send(at).unwrap();
                std::future::ready(())
            },
        )
        .await;
        assert!(result.is_err());
        if !error {
            assert_eq!(observed.try_recv().unwrap(), 10);
        }
        assert!(observed.try_recv().is_err());
        assert_eq!(states[0].cursor, 9);
    }
}

struct Sink {
    rejected: Notify,
    frames: mpsc::Sender<PushEnvelope>,
}

impl PushSink for Sink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        let envelope: PushEnvelope = rmp_serde::from_slice(&frame).unwrap();
        if envelope.push == PushKind::Event && envelope.seq == 1 {
            self.rejected.notify_one();
            return Err(PushRejected);
        }
        self.frames.try_send(envelope).unwrap();
        Ok(())
    }
}

// Real push retry helper inside the real drain, not an independent cursor model.
#[tokio::test(start_paused = true)]
async fn rejected_second_frame_does_not_advance_event_cursor() {
    let source = Source::new(vec![(10, page(&[10, 11], 11, 11, true))]);
    let (_tx, rx) = broadcast::channel(4);
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(9)];
    let (frames, mut observed) = mpsc::channel(4);
    let sink = Arc::new(Sink {
        rejected: Notify::new(),
        frames,
    });
    let push: Arc<dyn PushSink> = sink.clone();
    let seq = std::sync::atomic::AtomicU64::new(0);
    let result = JournalDrain::run(
        &source,
        &repos,
        &mut [rx],
        &mut states,
        true,
        |_, event| {
            let push = &push;
            let seq = &seq;
            async move {
                assert_eq!(event.commit_version, 10, "later transaction must not run");
                for data in [
                    b"first".as_slice(),
                    b"second".as_slice(),
                    b"third".as_slice(),
                ] {
                    if !try_push_event(push, 1, seq, data).await {
                        return false;
                    }
                }
                true
            }
        },
        |_| async { panic!("unexpected gap") },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(states[0].cursor, 9);
    assert_eq!(
        observed.try_recv().unwrap().data.as_deref(),
        Some(b"first".as_slice()),
    );
    assert_eq!(observed.try_recv().unwrap().push, PushKind::SlowConsumer);
    assert!(observed.try_recv().is_err());
}

// Red with periodic catch-up polling; virtual time cannot trigger a clean read.
#[tokio::test(start_paused = true)]
async fn clean_idle_repo_does_not_read_or_wait_for_progress_again() {
    let source = Source::new(vec![(10, page(&[], 9, 9, true))]);
    let (_tx, rx) = broadcast::channel(4);
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(9)];
    let mut receivers = [rx];
    let drain = JournalDrain::run(
        &source,
        &repos,
        &mut receivers,
        &mut states,
        false,
        |_, _| async { panic!("idle delivery") },
        |_| async { panic!("idle gap") },
    );
    tokio::pin!(drain);
    // Drive until the drain reports the clean receiver-only waiting state.
    tokio::select! {
        result = &mut drain => panic!("idle drain terminated: {result:?}"),
        () = source.idle.notified() => {}
    }
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!((&mut drain).now_or_never().is_none());
    assert_eq!(source.reads.load(Ordering::Relaxed), 1);
    assert_eq!(source.waits.load(Ordering::Relaxed), 0);
}

struct SnapshotSink {
    entered: Notify,
    released: AtomicBool,
    frames: mpsc::Sender<PushEnvelope>,
}

impl PushSink for SnapshotSink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        let envelope: PushEnvelope = rmp_serde::from_slice(&frame).unwrap();
        if !self.released.load(Ordering::Acquire) {
            self.entered.notify_one();
            return Err(PushRejected);
        }
        self.frames.try_send(envelope).unwrap();
        Ok(())
    }
}

// Red with post-snapshot last_committed seeding: the insert below is absent
// from the already-read snapshot and gets dropped from the live stream.
#[tokio::test(start_paused = true)]
async fn snapshot_handoff_uses_pre_query_cut_and_preserves_ready_order() {
    let db = Arc::new(ShamirDb::init_memory().await.unwrap());
    db.create_db("test_db").await;
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("test_db", config).await.unwrap();
    let mut seed = Batch::new();
    seed.insert("seed", insert("users").row(doc! { "_id" => "seed" }));
    db.execute_as(Actor::System, "test_db", &seed.build())
        .await
        .unwrap();
    let cut = db.changelog_visible_floor("test_db", "main").await.unwrap();
    let registry = Arc::new(SubscriptionRegistry::new());
    registry.try_reserve().unwrap();
    registry.reserve_pending(1);
    let (frames, mut observed) = mpsc::channel(16);
    let sink = Arc::new(SnapshotSink {
        entered: Notify::new(),
        released: AtomicBool::new(false),
        frames,
    });
    let push: Arc<dyn PushSink> = sink.clone();
    let mut bridge = Box::pin(bridge_task(
        1,
        db.clone(),
        "test_db".to_owned(),
        vec![SourceBuilder::table("users").build()],
        DeliverMode::Keys,
        Actor::System,
        push,
        None,
        true,
        BatchLimits::default(),
        registry.clone(),
    ));
    tokio::select! {
        biased;
        () = &mut bridge => panic!("bridge terminated before snapshot rejection"),
        () = sink.entered.notified() => {}
    }
    // The bridge future is not polled during this write, even if paused time
    // advances past its retry deadline. The snapshot has already been read.
    let mut write = Batch::new();
    write.insert("later", insert("users").row(doc! { "_id" => "later" }));
    db.execute_as(Actor::System, "test_db", &write.build())
        .await
        .unwrap();
    sink.released.store(true, Ordering::Release);
    let snapshot = tokio::select! {
        biased;
        () = &mut bridge => panic!("bridge terminated before snapshot delivery"),
        frame = observed.recv() => frame.expect("snapshot frame"),
    };
    assert_eq!(snapshot.push, PushKind::Event);
    let ready = tokio::select! {
        biased;
        () = &mut bridge => panic!("bridge terminated before ready delivery"),
        frame = observed.recv() => frame.expect("ready frame"),
    };
    assert_eq!(ready.push, PushKind::Ready);
    let live = tokio::select! {
        biased;
        () = &mut bridge => panic!("bridge terminated before live delivery"),
        frame = observed.recv() => frame.expect("live frame"),
    };
    assert_eq!(live.push, PushKind::Event);
    #[derive(serde::Deserialize)]
    struct Version {
        commit_version: u64,
    }
    let version: Version = rmp_serde::from_slice(live.data.as_deref().unwrap()).unwrap();
    assert!(version.commit_version > cut);
    let snapshot_version: Version =
        rmp_serde::from_slice(snapshot.data.as_deref().unwrap()).unwrap();
    assert_eq!(snapshot_version.commit_version, 0);
    assert_eq!((snapshot.seq, ready.seq, live.seq), (0, 1, 2));
    // Drop the owning pinned future, not just a borrowed Pin: the bridge's
    // registry guard must run synchronously on cancellation.
    drop(bridge);
    assert_eq!(registry.count(), 0);
}

// One subscriber blocked in required-frame retry must not stall another drain.
#[tokio::test(start_paused = true)]
async fn subscribers_with_distinct_cursors_are_isolated_from_rejected_sink() {
    let slow_source = Source::new(vec![(10, page(&[10], 11, 11, true))]);
    let fast_source = Source::new(vec![
        (11, page(&[11], 11, 11, true)),
        (12, page(&[], 11, 11, true)),
    ]);
    let (_slow_tx, slow_rx) = broadcast::channel(4);
    let (_fast_tx, fast_rx) = broadcast::channel(4);
    let repos = vec!["main".to_owned()];
    let mut slow_states = vec![JournalDrain::new(9)];
    let mut fast_states = vec![JournalDrain::new(10)];
    let mut slow_receivers = [slow_rx];
    let mut fast_receivers = [fast_rx];
    let (frames, mut slow_frames) = mpsc::channel(4);
    let sink = Arc::new(Sink {
        rejected: Notify::new(),
        frames,
    });
    let push: Arc<dyn PushSink> = sink.clone();
    let seq = std::sync::atomic::AtomicU64::new(1);
    let slow = JournalDrain::run(
        &slow_source,
        &repos,
        &mut slow_receivers,
        &mut slow_states,
        true,
        |_, _| try_push_event(&push, 1, &seq, b"blocked"),
        |_| async { panic!("unexpected gap") },
    );
    tokio::pin!(slow);
    tokio::select! {
        result = &mut slow => panic!("slow drain terminated before rejection: {result:?}"),
        () = sink.rejected.notified() => {}
    }
    let (out, mut observed) = mpsc::channel(4);
    JournalDrain::run(
        &fast_source,
        &repos,
        &mut fast_receivers,
        &mut fast_states,
        true,
        |_, event| {
            out.try_send(event.commit_version).unwrap();
            std::future::ready(true)
        },
        |_| async { panic!("unexpected gap") },
    )
    .await
    .unwrap();
    assert_eq!(observed.try_recv().unwrap(), 11);
    assert_eq!(fast_states[0].cursor, 11);
    assert!(slow_frames.try_recv().is_err(), "slow sink remains blocked");
    assert!(slow.await.is_err());
    assert_eq!(slow_frames.try_recv().unwrap().push, PushKind::SlowConsumer);
}

// A's delayed broadcast is released only AFTER both journal events reached
// the observer. The broadcast payload must never become a second delivery.
#[tokio::test]
async fn late_older_broadcast_after_delivery_does_not_duplicate() {
    let source = Source::new(vec![
        (10, page(&[10, 11], 11, 11, true)),
        (12, page(&[], 11, 11, true)),
    ]);
    let (tx, rx) = broadcast::channel(4);
    let repos = vec!["main".to_owned()];
    let mut states = vec![JournalDrain::new(9)];
    let mut receivers = [rx];
    let (out, mut observed) = mpsc::channel(4);
    let drain = JournalDrain::run(
        &source,
        &repos,
        &mut receivers,
        &mut states,
        false,
        |_, event| {
            out.try_send(event.commit_version).unwrap();
            std::future::ready(true)
        },
        |_| async { panic!("unexpected gap") },
    );
    tokio::pin!(drain);
    tokio::select! {
        result = &mut drain => panic!("idle drain terminated: {result:?}"),
        () = source.idle.notified() => {}
    }
    assert_eq!(observed.try_recv().unwrap(), 10);
    assert_eq!(observed.try_recv().unwrap(), 11);
    source
        .pages
        .lock()
        .unwrap()
        .push_back((12, Ok(page(&[], 11, 11, true))));
    tx.send(Arc::new(event(10))).unwrap();
    tokio::select! {
        result = &mut drain => panic!("idle drain terminated: {result:?}"),
        () = source.idle.notified() => {}
    }
    assert!(observed.try_recv().is_err());
    assert_eq!(source.reads.load(Ordering::Relaxed), 3);
}

// Pending completion for one repo is multiplexed, not awaited inline while
// another repo has safe history to deliver.
#[tokio::test]
async fn progress_wait_does_not_block_another_repository() {
    let mut source = Source::new(vec![
        (10, page(&[], 9, 9, true)),
        (21, page(&[21], 21, 21, true)),
        (10, page(&[], 9, 9, true)),
        (22, page(&[], 21, 21, true)),
        (10, page(&[11], 11, 11, true)),
        (12, page(&[], 11, 11, true)),
    ]);
    source.park_progress = true;
    let (tx_a, rx_a) = broadcast::channel(4);
    let (_tx_b, rx_b) = broadcast::channel(4);
    tx_a.send(Arc::new(event(11))).unwrap();
    let repos = vec!["a".to_owned(), "b".to_owned()];
    let mut states = vec![JournalDrain::new(9), JournalDrain::new(20)];
    let mut receivers = [rx_a, rx_b];
    let (out, mut observed) = mpsc::channel(4);
    let drain = JournalDrain::run(
        &source,
        &repos,
        &mut receivers,
        &mut states,
        true,
        |ri, event| {
            out.try_send((ri, event.commit_version)).unwrap();
            std::future::ready(true)
        },
        |_| async { panic!("unexpected gap") },
    );
    tokio::pin!(drain);
    tokio::select! {
        result = &mut drain => panic!("premature completion: {result:?}"),
        () = source.entered.notified() => {}
    }
    assert_eq!(observed.try_recv().unwrap(), (1, 21));
    assert!(observed.try_recv().is_err());
    source.release.notify_one();
    drain.await.unwrap();
    assert_eq!(observed.try_recv().unwrap(), (0, 11));
    assert!(observed.try_recv().is_err());
}

// Exercises the production target/filter/cache path on real journal pages,
// with 16 independent subscribers. A later matching event is the observable
// fence proving the intervening filtered event was processed, not just delayed.
#[tokio::test]
async fn many_real_bridges_reuse_filters_and_resume_inclusively() {
    let db = Arc::new(ShamirDb::init_memory().await.unwrap());
    db.create_db("test_db").await;
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("test_db", config).await.unwrap();
    let registry = Arc::new(SubscriptionRegistry::with_cap(32));
    let mut handles = Vec::new();
    let mut receivers = Vec::new();
    for id in 1..=16 {
        registry.try_reserve().unwrap();
        registry.reserve_pending(id);
        let (frames, mut observed) = mpsc::channel(16);
        let push: Arc<dyn PushSink> = Arc::new(SnapshotSink {
            entered: Notify::new(),
            released: AtomicBool::new(true),
            frames,
        });
        let source = if id % 2 == 0 {
            SourceBuilder::table("users").filter(eq("group", 7)).build()
        } else {
            SourceBuilder::table("users").build()
        };
        handles.push(tokio::spawn(bridge_task(
            id,
            db.clone(),
            "test_db".to_owned(),
            vec![source],
            DeliverMode::Keys,
            Actor::System,
            push,
            None,
            true,
            BatchLimits::default(),
            registry.clone(),
        )));
        assert_eq!(observed.recv().await.unwrap().push, PushKind::Ready);
        receivers.push(observed);
    }
    let mut versions = Vec::new();
    for (key, group) in [("a", 7), ("b", 8), ("c", 7)] {
        let mut write = Batch::new();
        write.insert(
            "row",
            insert("users").row(doc! {
                "_id" => key,
                "group" => group
            }),
        );
        db.execute_as(Actor::System, "test_db", &write.build())
            .await
            .unwrap();
        versions.push(db.changelog_visible_floor("test_db", "main").await.unwrap());
    }
    #[derive(serde::Deserialize)]
    struct Version {
        commit_version: u64,
    }
    for (index, receiver) in receivers.iter_mut().enumerate() {
        let expected = if index % 2 == 1 {
            vec![versions[0], versions[2]]
        } else {
            versions.clone()
        };
        for (offset, version) in expected.into_iter().enumerate() {
            let frame = receiver.recv().await.unwrap();
            assert_eq!(frame.push, PushKind::Event);
            let payload: Version = rmp_serde::from_slice(frame.data.as_deref().unwrap()).unwrap();
            assert_eq!(payload.commit_version, version);
            assert_eq!(frame.seq, u64::try_from(offset).unwrap() + 1);
        }
    }
    // Both zero (all journal history) and a positive inclusive boundary must
    // replay already-present history without needing another payload hint.
    for (id, from, expected) in [
        (17, 0, versions.as_slice()),
        (18, versions[1], &versions[1..]),
    ] {
        registry.try_reserve().unwrap();
        registry.reserve_pending(id);
        let (frames, mut observed) = mpsc::channel(16);
        let push: Arc<dyn PushSink> = Arc::new(SnapshotSink {
            entered: Notify::new(),
            released: AtomicBool::new(true),
            frames,
        });
        handles.push(tokio::spawn(bridge_task(
            id,
            db.clone(),
            "test_db".to_owned(),
            vec![SourceBuilder::table("users").build()],
            DeliverMode::Keys,
            Actor::System,
            push,
            Some(from),
            false,
            BatchLimits::default(),
            registry.clone(),
        )));
        for version in expected {
            let frame = observed.recv().await.unwrap();
            let payload: Version = rmp_serde::from_slice(frame.data.as_deref().unwrap()).unwrap();
            assert_eq!(payload.commit_version, *version);
        }
    }
    for handle in handles {
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
    }
    assert_eq!(registry.count(), 0);
}
