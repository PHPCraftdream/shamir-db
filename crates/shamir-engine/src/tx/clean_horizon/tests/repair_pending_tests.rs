//! S5c-X PR5b part 2 — channel-full journal repair drainer + retention clamp
//! behaviour tests.
//!
//! All waiting is on CONDITIONS (Notify / atomic counters /
//! `last_persisted_version` / store probes) via bounded yield spins — never
//! wall-clock sleeps. Every spawned task is joined; the parked journal
//! writer is always released (or deliberately leaked only across a hard
//! reopen, where the test runtime drop reaps it) and every temp dir is
//! cleaned by `TempDir`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_tx::changefeed::cold::encode_event;
use shamir_tx::changefeed::version_key;
use shamir_tx::{
    ChangelogEvent, ChangelogStore, IsolationLevel, RepoChangefeed, JOURNAL_CHANNEL_CAPACITY,
};
use shamir_types::access::Actor;
use shamir_types::types::value::InnerValue;
use tokio::sync::Notify;

use crate::repo::changelog_store::StoreChangelog;
use crate::repo::repo_instance::ChangefeedHandle;
use crate::repo::{BoxRepo, BoxRepoFactory, DurabilityPolicy, RepoInstance};
use crate::table::TableConfig;
use crate::tx::clean_horizon::checkpoint::test_seam;

const MAX_KEY: u64 = u64::MAX;
const BACKLOG_KEY: u64 = u64::MAX - 1;

/// Journal store that parks the changefeed writer inside its FIRST `put` of
/// the `u64::MAX` filler (same mechanism as the orphan adversarial fixture):
/// with the writer parked mid-put and the channel filled to capacity, every
/// further `journal_enqueue` deterministically returns Full and defers.
struct ParkStore {
    inner: StoreChangelog,
    event_puts: AtomicUsize,
    block_filler_once: AtomicBool,
    filler_entered: Notify,
    release_filler: Notify,
}

impl ParkStore {
    fn new() -> Self {
        Self {
            inner: StoreChangelog::new(Arc::new(InMemoryStore::new())),
            event_puts: AtomicUsize::new(0),
            block_filler_once: AtomicBool::new(false),
            filler_entered: Notify::new(),
            release_filler: Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl ChangelogStore for ParkStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String> {
        if key.len() == 8 {
            if key.as_ref() == MAX_KEY.to_be_bytes().as_slice()
                && self.block_filler_once.swap(false, Ordering::AcqRel)
            {
                self.filler_entered.notify_one();
                self.release_filler.notified().await;
            }
            self.event_puts.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.put(key, value).await
    }

    async fn range_from(&self, key: Bytes, limit: usize) -> Result<Vec<Bytes>, String> {
        self.inner.range_from(key, limit).await
    }
}

fn park_store() -> Arc<ParkStore> {
    Arc::new(ParkStore::new())
}

fn journal_handle(store: &Arc<ParkStore>) -> ChangefeedHandle {
    let journal: Arc<dyn ChangelogStore> = Arc::clone(store);
    ChangefeedHandle {
        feed: RepoChangefeed::new(Arc::clone(&journal)),
        store: journal,
    }
}

/// In-memory fixture with the parking journal installed and the background
/// drainer disabled (single-owner: only the test drives `drain_all`).
async fn fixture(name: &str) -> (RepoInstance, Arc<RepoChangefeed>, Arc<ParkStore>) {
    let repo = RepoInstance::new(
        name.into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("a")],
    );
    repo.disable_background_drainer_for_test();
    let store = park_store();
    repo.install_journal_for_test(journal_handle(&store));
    let feed = repo.changefeed().await.unwrap().feed;
    (repo, feed, store)
}

/// Disk (fjall) fixture with a small segment cap so commits seal segments
/// deterministically; background drainer disabled.
async fn open_disk(
    name: &str,
    path: &std::path::Path,
) -> shamir_storage::error::DbResult<RepoInstance> {
    let repo = RepoInstance::from_factory_with_test_segment_bytes(
        name.into(),
        BoxRepoFactory::fjall_raw(path.to_path_buf()),
        vec![TableConfig::new("t")],
        DurabilityPolicy::default(),
        256,
    )
    .await?;
    repo.disable_background_drainer_for_test();
    Ok(repo)
}

/// Bounded-retry hard reopen (Windows releases the fjall file lock lazily
/// after `drop`); mirrors `tx/tests/recovery_tests.rs::reopen_disk_repo`.
async fn open_retry(name: &str, path: &std::path::Path) -> RepoInstance {
    let mut last_err = None;
    for _attempt in 0..10 {
        match open_disk(name, path).await {
            Ok(repo) => return repo,
            Err(error) => last_err = Some(error),
        }
    }
    panic!("reopen of {name:?} failed after retries: {:?}", last_err);
}

fn filler_event(version: u64) -> Arc<ChangelogEvent> {
    Arc::new(ChangelogEvent {
        repo: "filler".into(),
        commit_version: version,
        tx_id: 0,
        actor: Actor::System,
        timestamp_ns: 0,
        changes: vec![],
    })
}

/// Park the journal writer inside its first put and fill the channel to
/// capacity: every subsequent `journal_enqueue` deterministically hits Full.
async fn park_writer(feed: &RepoChangefeed, store: &Arc<ParkStore>) {
    store.block_filler_once.store(true, Ordering::Release);
    feed.journal_enqueue(&filler_event(MAX_KEY));
    store.filler_entered.notified().await;
    let queued = filler_event(BACKLOG_KEY);
    for _ in 0..JOURNAL_CHANNEL_CAPACITY {
        feed.journal_enqueue(&queued);
    }
}

async fn commit_row(repo: &RepoInstance, table: &crate::table::TableManager, tag: &str) -> u64 {
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str(tag.into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap().commit_version
}

/// Bounded condition wait (no wall-clock sleeps): spin yields until the
/// journal writer has persisted at least `through`.
async fn wait_persisted(feed: &RepoChangefeed, through: u64) {
    for _ in 0..200_000 {
        if feed.last_persisted_version() >= through {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("journal writer never persisted through {through}");
}

/// Bounded condition wait until the event at `version` is readable from the
/// journal store.
async fn wait_event_stored(store: &Arc<ParkStore>, version: u64) {
    for _ in 0..200_000 {
        let rows = store.range_from(version_key(version), 1).await.unwrap();
        if let Some(bytes) = rows.first() {
            if let Ok(event) = rmp_serde::from_slice::<ChangelogEvent>(bytes) {
                if event.commit_version == version {
                    return;
                }
            }
        }
        tokio::task::yield_now().await;
    }
    panic!("event at {version} never reached the journal store");
}

/// Drain passes until every repair-pending version is retired.
async fn drain_until_empty(repo: &RepoInstance, feed: &RepoChangefeed) {
    for _ in 0..100 {
        repo.drainer().drain_all(repo).await.unwrap();
        if feed.repair_pending_depth() == 0 {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "repair-pending set never emptied (depth {})",
        feed.repair_pending_depth()
    );
}

// (a) Channel-full commits are deferred WITHOUT a gap and the repair drainer
// lands them in ascending version order; a follower pull sees every event
// exactly once, contiguous, `gap == None`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_full_defers_and_repairs_in_order() {
    let (repo, feed, store) = fixture("rp_full_channel").await;
    let table = repo.get_table("a").await.unwrap();
    park_writer(&feed, &store).await;
    let base = repo.current_commit_version().await.unwrap();
    let mut versions = Vec::new();
    for index in 0..5 {
        versions.push(commit_row(&repo, &table, &format!("row-{index}")).await);
    }
    // NO gap: channel-full defers, it does not drop.
    assert_eq!(feed.first_gap_version(), 0);
    assert_eq!(feed.repair_pending_depth(), 5);
    assert_eq!(feed.channel_full_total(), 5);

    store.release_filler.notify_one();
    wait_persisted(&feed, BACKLOG_KEY).await;
    drain_until_empty(&repo, &feed).await;
    assert_eq!(feed.repaired_total(), 5);
    assert_eq!(feed.first_gap_version(), 0);

    wait_event_stored(&store, versions[4]).await;
    let (events, gap) = repo
        .read_changelog_for_replication(base + 1, 64)
        .await
        .unwrap();
    assert_eq!(gap, None);
    let real: Vec<u64> = events
        .iter()
        .map(|event| event.event.commit_version)
        .filter(|version| *version <= versions[4])
        .collect();
    assert_eq!(
        real,
        (base + 1..=versions[4]).collect::<Vec<u64>>(),
        "pull must return every deferred event exactly once, ascending"
    );
    feed.shutdown_and_wait().await.unwrap();
}

// (b) Retention gate: the sealed WAL segment holding a repair-pending
// version stays retained (clean horizon does not pass the hole) until the
// repaired event is checkpointed — and the negative control proves the clamp
// is load-bearing (without it the truncation destroys the pending evidence).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_gate_holds_segment_until_checkpointed() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("rp_retention");
    let repo = open_disk("rp_retention", &path).await.unwrap();
    let table = repo.get_table("t").await.unwrap();
    for index in 0..4 {
        commit_row(&repo, &table, &format!("seed-{index}-{}", "x".repeat(64))).await;
    }
    repo.repo_interner().await.unwrap().persist().await.unwrap();
    repo.drainer().drain_all(&repo).await.unwrap();

    // Install the parking journal AFTER the baseline drain: the parked
    // writer starts from an empty channel.
    let store = park_store();
    repo.install_journal_for_test(journal_handle(&store));
    let feed = repo.changefeed().await.unwrap().feed;
    park_writer(&feed, &store).await;

    let v = commit_row(&repo, &table, &format!("held-{}", "y".repeat(64))).await;
    assert_eq!(feed.repair_pending_depth(), 1);
    assert_eq!(feed.first_gap_version(), 0);
    // Force enough frames to seal the segment holding `v`.
    let mut last = v;
    for index in 0..6 {
        last = commit_row(&repo, &table, &format!("seal-{index}-{}", "z".repeat(96))).await;
    }
    assert_eq!(feed.repair_pending_depth(), 7);

    let group = repo.wal_group().await.unwrap();
    repo.repo_interner().await.unwrap().persist().await.unwrap();

    // Clamped pass: the ceiling may not cross the still-pending version.
    repo.drainer().drain_all(&repo).await.unwrap();
    assert!(
        group.has_truncatable(last),
        "a sealed, fully durable segment crossing v must be truncatable but for \
         the clamp — otherwise this test proves nothing"
    );
    assert!(group.sealed_retained_bytes().await.unwrap() > 0);
    assert!(
        group
            .replay()
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.commit_version == v),
        "the clamped ceiling must retain the WAL entry of the pending version"
    );
    assert!(feed.clean_horizon() < v);

    // NEGATIVE CONTROL: disable the clamp (and bypass the clean-horizon
    // checkpoint — the parked writer cannot accept its CleanHorizon message)
    // and the same truncation now crosses the pending version, destroying
    // the WAL evidence of a version that was never repaired.
    feed.repair_pending().test_set_clamp_disabled();
    test_seam::with_state(&repo, |state| state.bypass_checkpoint = true);
    let mut gone = false;
    for _ in 0..500 {
        repo.drainer().drain_all(&repo).await.unwrap();
        gone = !group
            .replay()
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.commit_version == v);
        if gone {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        gone,
        "without the clamp the truncation crosses the still-pending version"
    );
    assert_eq!(
        feed.repair_pending_depth(),
        7,
        "the pending versions were never repaired — the unclamped truncation \
         destroyed their only re-derivation source"
    );

    store.release_filler.notify_one();
    wait_persisted(&feed, BACKLOG_KEY).await;
    feed.shutdown_and_wait().await.unwrap();
}

// (c) A version left repair-pending across a HARD restart is rebuilt from
// the retained WAL by the cold reconciliation; no gap, depth 0 after drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unclean_restart_rebuilds_deferred_event() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("rp_restart");
    let repo = open_disk("rp_restart", &path).await.unwrap();
    let table = repo.get_table("t").await.unwrap();
    let store = park_store();
    repo.install_journal_for_test(journal_handle(&store));
    let feed = repo.changefeed().await.unwrap().feed;
    park_writer(&feed, &store).await;
    let v = commit_row(&repo, &table, "deferred").await;
    assert_eq!(feed.repair_pending_depth(), 1);
    assert_eq!(feed.first_gap_version(), 0);
    // HARD drop: the writer stays parked (never released, never shut down);
    // the test runtime reaps it.
    drop(repo);

    for round in 0..2 {
        let repo = open_retry("rp_restart", &path).await;
        repo.disable_background_drainer_for_test();
        repo.drainer().drain_all(&repo).await.unwrap();
        let feed = repo.changefeed().await.unwrap().feed;
        assert_eq!(feed.repair_pending_depth(), 0, "round {round}");
        assert_eq!(feed.first_gap_version(), 0, "round {round}");
        let (events, gap) = repo.read_changelog_for_replication(v, 8).await.unwrap();
        assert_eq!(gap, None, "round {round}");
        assert!(
            events.iter().any(|event| event.event.commit_version == v),
            "round {round}: cold reconciliation must rebuild the deferred event \
             from the retained WAL"
        );
        feed.shutdown_and_wait().await.unwrap();
        drop(repo);
    }
}

// (d) A pending version whose WAL entry is legacy (pre-header v2 wire
// format, event underivable) declares the minimal gap and retires.
#[derive(serde::Serialize)]
struct LegacyV2Shape {
    txn_id: u64,
    repo_id_interned: u64,
    started_at_ns: u64,
    commit_version: u64,
    ops: Vec<shamir_wal::WalOpV2>,
    interner_delta: Vec<(u64, String, u64)>,
}

#[tokio::test]
async fn legacy_entry_declares_minimal_gap() {
    let (repo, feed, _store) = fixture("rp_legacy").await;
    let v = repo.current_commit_version().await.unwrap() + 1;
    let body = LegacyV2Shape {
        txn_id: 77,
        repo_id_interned: 0,
        started_at_ns: 0,
        commit_version: v,
        ops: vec![],
        interner_delta: vec![],
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&shamir_wal::WAL_V2_MAGIC);
    bytes.push(2); // pre-event format version
    bincode::serialize_into(&mut bytes, &body).unwrap();
    repo.wal_group()
        .await
        .unwrap()
        .append(bytes, v, shamir_wal::WalDurability::Buffered)
        .await
        .unwrap();
    feed.repair_pending().record_channel_full(v);

    repo.drainer().drain_all(&repo).await.unwrap();
    assert_eq!(
        feed.first_gap_version(),
        v,
        "legacy evidence declares a gap"
    );
    assert_eq!(feed.repair_pending_depth(), 0);
    assert_eq!(feed.repair_mismatch_total(), 0);
    feed.shutdown_and_wait().await.unwrap();
}

// (e) Re-running the repair pass over an already-present, equal journal
// event retires the version WITHOUT a second store put (idempotence).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn double_repair_produces_one_put() {
    let (repo, feed, store) = fixture("rp_double").await;
    let table = repo.get_table("a").await.unwrap();
    park_writer(&feed, &store).await;
    let v = commit_row(&repo, &table, "once").await;
    assert_eq!(feed.repair_pending_depth(), 1);

    store.release_filler.notify_one();
    wait_persisted(&feed, BACKLOG_KEY).await;
    // The filler + backlog puts also count; snapshot them as the baseline.
    let puts_baseline = store.event_puts.load(Ordering::Relaxed);
    repo.drainer().drain_all(&repo).await.unwrap();
    assert_eq!(feed.repair_pending_depth(), 0);
    wait_event_stored(&store, v).await;
    assert_eq!(
        store.event_puts.load(Ordering::Relaxed),
        puts_baseline + 1,
        "the repair must produce exactly one event put"
    );

    // Second repair pass over the SAME version: the store probe finds the
    // event present and equal — retire, never re-put.
    feed.repair_pending().record_channel_full(v);
    repo.drainer().drain_all(&repo).await.unwrap();
    assert_eq!(feed.repair_pending_depth(), 0);
    assert_eq!(
        store.event_puts.load(Ordering::Relaxed),
        puts_baseline + 1,
        "the second pass must not re-put the already-present event"
    );
    assert_eq!(feed.repair_mismatch_total(), 0);
    assert_eq!(feed.first_gap_version(), 0);
    feed.shutdown_and_wait().await.unwrap();
}

// (f) A journal slot holding a DIFFERENT event than the WAL-derived one is a
// loud repair-mismatch: counted, never overwritten, never a gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatching_event_is_loud_error() {
    let (repo, feed, _store) = fixture("rp_mismatch").await;
    let table = repo.get_table("a").await.unwrap();
    let v = commit_row(&repo, &table, "truth").await;
    wait_persisted(&feed, v).await;

    let alien = Arc::new(ChangelogEvent {
        repo: "alien".into(),
        commit_version: v,
        tx_id: 42,
        actor: Actor::System,
        timestamp_ns: 7,
        changes: vec![],
    });
    let encoded = encode_event(&alien).unwrap();
    let handle = repo.changefeed().await.unwrap();
    handle
        .store
        .put(version_key(v), encoded.clone())
        .await
        .unwrap();

    feed.repair_pending().record_channel_full(v);
    repo.drainer().drain_all(&repo).await.unwrap();

    assert_eq!(feed.repair_mismatch_total(), 1);
    assert_eq!(feed.repair_pending_depth(), 0);
    assert_eq!(
        feed.first_gap_version(),
        0,
        "the journal HAS an event — no gap"
    );
    let read_back = handle.store.range_from(version_key(v), 1).await.unwrap();
    assert_eq!(read_back.first(), Some(&encoded), "never overwrite");
    feed.shutdown_and_wait().await.unwrap();
}

// (g) Interleaved normal commits + repaired deferrals produce one contiguous
// ascending version sequence; during the pending window a bounded pull sees
// nothing at or above the hole (barrier/ceiling clamp).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interleaved_normal_commits_keep_order() {
    let (repo, feed, store) = fixture("rp_interleaved").await;
    let table = repo.get_table("a").await.unwrap();
    park_writer(&feed, &store).await;
    let base = repo.current_commit_version().await.unwrap();
    let mut pending_v = Vec::new();
    for index in 0..5 {
        pending_v.push(commit_row(&repo, &table, &format!("p-{index}")).await);
    }
    let min_pending = feed.repair_pending_min().unwrap();
    assert_eq!(min_pending, pending_v[0]);

    let handle = repo.changefeed().await.unwrap();
    let bounded = handle
        .feed
        .read_from_bounded(&handle.store, base + 1, 100, min_pending - 1)
        .await;
    assert_eq!(bounded.gap_at, None);
    assert!(
        bounded
            .events
            .iter()
            .all(|event| event.commit_version < min_pending),
        "a bounded pull during the pending window must not cross the hole"
    );

    store.release_filler.notify_one();
    wait_persisted(&feed, BACKLOG_KEY).await;
    let mut normal = Vec::new();
    for index in 0..2 {
        normal.push(commit_row(&repo, &table, &format!("n-{index}")).await);
    }
    drain_until_empty(&repo, &feed).await;
    wait_event_stored(&store, normal[1]).await;

    let (events, gap) = repo
        .read_changelog_for_replication(base + 1, 100)
        .await
        .unwrap();
    assert_eq!(gap, None);
    let top = normal[1];
    let got: Vec<u64> = events
        .iter()
        .map(|event| event.event.commit_version)
        .filter(|version| *version <= top)
        .collect();
    let mut expected = pending_v;
    expected.extend_from_slice(&normal);
    assert_eq!(
        got, expected,
        "repaired deferrals + normal commits must form one contiguous \
         ascending sequence"
    );
    feed.shutdown_and_wait().await.unwrap();
}
