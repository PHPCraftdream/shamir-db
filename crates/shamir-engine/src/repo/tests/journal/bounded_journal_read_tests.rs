use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::poll;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_tx::changefeed::version_key;
use shamir_tx::completion_tracker::State;
use shamir_tx::repo_tx_gate::VisibleOutcome;
use shamir_tx::{ChangeOp, ChangelogEvent, ChangelogStore, RecordChange, RepoChangefeed};
use shamir_types::access::Actor;
use tokio::sync::Notify;

use crate::repo::changelog_store::StoreChangelog;
use crate::repo::repo_instance::ChangefeedHandle;
use crate::repo::{BoxRepo, RepoInstance};
use shamir_tx::ChangelogError;

// Uses the real ordered, byte-budgeted adapter; only write admission and
// explicit range failure are injected. No clock-based scheduling assumptions.
struct JournalFixture {
    inner: StoreChangelog,
    block_put: AtomicBool,
    fail_read: AtomicBool,
    reached: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl ChangelogStore for JournalFixture {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), ChangelogError> {
        if self.block_put.swap(false, Ordering::AcqRel) {
            self.reached.notify_one();
            self.resume.notified().await;
        }
        self.inner.put(key, value).await
    }

    async fn range_from(
        &self,
        from_key: Bytes,
        limit: usize,
    ) -> Result<Vec<Bytes>, ChangelogError> {
        if self.fail_read.load(Ordering::Acquire) {
            return Err(ChangelogError::Journal(
                "injected journal range failure".into(),
            ));
        }
        self.inner.range_from(from_key, limit).await
    }
}

fn event(version: u64) -> ChangelogEvent {
    ChangelogEvent {
        repo: "bounded-journal".into(),
        commit_version: version,
        tx_id: version,
        actor: Actor::System,
        timestamp_ns: 0,
        changes: vec![],
    }
}

async fn fixture() -> (RepoInstance, Arc<JournalFixture>, Arc<RepoChangefeed>) {
    let repo = RepoInstance::new(
        "bounded-journal".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![],
    );
    let store = Arc::new(JournalFixture {
        inner: StoreChangelog::new(Arc::new(InMemoryStore::new())),
        block_put: AtomicBool::new(false),
        fail_read: AtomicBool::new(false),
        reached: Notify::new(),
        resume: Notify::new(),
    });
    let journal: Arc<dyn ChangelogStore> = store.clone();
    let feed = RepoChangefeed::new(journal.clone());
    repo.install_journal_for_test(ChangefeedHandle {
        feed: feed.clone(),
        store: journal,
    });
    (repo, store, feed)
}

async fn seed(repo: &RepoInstance, feed: &RepoChangefeed, version: u64) {
    feed.emit_journal_only(event(version));
    repo.tx_gate()
        .await
        .unwrap()
        .completion()
        .mark(version, State::Materialized);
}

// Baseline has no exposed B/W/outcome. Catch latest-version sampling and
// metadata inferred from the last event, including settled empty versions.
#[tokio::test]
async fn confirmed_bound_covers_empty_versions_and_legacy_page_matches() {
    let (repo, _, feed) = fixture().await;
    seed(&repo, &feed, 1).await;
    let gate = repo.tx_gate().await.unwrap();
    gate.completion().mark(2, State::Aborted);
    let page = repo.read_changelog_bounded(1, 10).await.unwrap();
    assert_eq!((page.sampled_w, page.bound, page.confirmed), (2, 2, true));
    assert_eq!(page.read.events, vec![event(1)]);
    assert_eq!(page.read.gap_at, None);
    let legacy = repo.read_changelog_from(1, 10).await.unwrap();
    assert_eq!(legacy.events, page.read.events);
    assert_eq!(legacy.gap_at, page.read.gap_at);
    let empty = repo.read_changelog_bounded(2, 10).await.unwrap();
    assert!(empty.read.events.is_empty());
    assert_eq!(empty.read.gap_at, None);
    assert_eq!(
        (empty.sampled_w, empty.bound, empty.confirmed),
        (2, 2, true)
    );
    feed.shutdown();
}

// A pending completion slot below published/persisted B must cap the page
// at W, while confirmation remains true (not "caught up to latest").
#[tokio::test]
async fn parked_a_below_b_uses_completion_not_latest() {
    let (repo, _, feed) = fixture().await;
    seed(&repo, &feed, 1).await;
    let gate = repo.tx_gate().await.unwrap();
    // Leave version 2 missing/unsettled: mark inserts rather than updates slots.
    feed.emit_journal_only(event(3));
    gate.raise_published(3);
    gate.completion().mark(3, State::Materialized);
    assert_eq!(feed.confirm_through(3, Duration::from_secs(1)).await, 3);
    let page = repo.read_changelog_bounded(1, 10).await.unwrap();
    assert_eq!((page.sampled_w, page.bound, page.confirmed), (1, 1, true));
    assert_eq!(page.read.events, vec![event(1)]);
    assert_eq!(page.read.gap_at, None);
    gate.completion().mark(2, State::Aborted);
    let after = repo.read_changelog_bounded(2, 10).await.unwrap();
    assert_eq!(after.bound, 3);
    assert_eq!(after.read.events, vec![event(3)]);
    feed.shutdown();
}

// Baseline returns an old page but cannot distinguish timeout from exhaustion.
#[tokio::test(start_paused = true)]
async fn confirmation_timeout_exposes_old_prefix_then_recovers_without_hint() {
    let (repo, store, feed) = fixture().await;
    seed(&repo, &feed, 1).await;
    assert_eq!(repo.read_changelog_bounded(1, 10).await.unwrap().bound, 1);
    store.block_put.store(true, Ordering::Release);
    seed(&repo, &feed, 2).await;
    store.reached.notified().await;
    let pending = repo.read_changelog_bounded(1, 10);
    tokio::pin!(pending);
    assert!(poll!(pending.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    let page = pending.await.unwrap();
    assert_eq!((page.sampled_w, page.bound, page.confirmed), (2, 1, false));
    assert_eq!(page.read.events, vec![event(1)]);
    assert_eq!(page.read.gap_at, None);
    assert_eq!(feed.confirm_timeouts(), 1);
    store.resume.notify_one();
    let recovered = repo.read_changelog_bounded(2, 10).await.unwrap();
    assert_eq!((recovered.bound, recovered.confirmed), (2, true));
    assert_eq!(recovered.read.events, vec![event(2)]);
    feed.shutdown();
}

async fn settlement_releases_wait(state: State) {
    let (repo, _, feed) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    // Leave version 1 missing/unsettled until its terminal state is inserted.
    gate.completion().mark(2, State::Materialized);
    // Poll, cancel, recreate: registration must not alter the target or
    // completion state, and no new broadcast is necessary to release it.
    {
        let cancelled = repo.await_journal_progress(2);
        tokio::pin!(cancelled);
        assert!(poll!(cancelled.as_mut()).is_pending());
    }
    let wait = repo.await_journal_progress(2);
    tokio::pin!(wait);
    assert!(poll!(wait.as_mut()).is_pending());
    gate.completion().mark(1, state);
    assert_eq!(wait.await.unwrap(), VisibleOutcome::Visible);
    assert_eq!(gate.visible_floor(), 2);
    feed.shutdown();
}

#[tokio::test]
async fn progress_wait_releases_on_complete_without_payload_hint() {
    settlement_releases_wait(State::Materialized).await;
}

#[tokio::test]
async fn progress_wait_releases_on_abort_without_payload_hint() {
    settlement_releases_wait(State::Aborted).await;
}

#[tokio::test(start_paused = true)]
async fn progress_wait_reports_reopen_after_parking() {
    let (repo, _, feed) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    let wait = repo.await_journal_progress(1);
    tokio::pin!(wait);
    assert!(poll!(wait.as_mut()).is_pending());
    gate.set_requires_reopen();
    tokio::time::advance(Duration::from_millis(50)).await;
    assert_eq!(wait.await.unwrap(), VisibleOutcome::ReopenRequired);
    assert_eq!(gate.visible_floor(), 0);
    feed.shutdown();
}

// An actual byte-limited short page must retain B, not pretend its last
// event is exhaustion. Only the following empty page finishes the prefix.
#[tokio::test]
async fn byte_limited_short_page_retains_confirmed_bound() {
    let (repo, store, feed) = fixture().await;
    let payload = Bytes::from(vec![0; 9 * 1024 * 1024]);
    for version in 1..=2 {
        let mut large = event(version);
        large.changes.push(RecordChange {
            table: "large".into(),
            key: version_key(version),
            op: ChangeOp::Put,
            value: Some(payload.clone()),
        });
        store
            .put(
                version_key(version),
                Bytes::from(rmp_serde::to_vec(&large).unwrap()),
            )
            .await
            .unwrap();
        repo.tx_gate()
            .await
            .unwrap()
            .completion()
            .mark(version, State::Materialized);
    }
    let first = repo.read_changelog_bounded(1, 10).await.unwrap();
    assert_eq!((first.bound, first.confirmed), (2, true));
    assert_eq!(first.read.events.len(), 1);
    assert_eq!(first.read.events[0].commit_version, 1);
    let second = repo.read_changelog_bounded(2, 10).await.unwrap();
    assert_eq!(second.read.events.len(), 1);
    assert_eq!(second.read.events[0].commit_version, 2);
    let empty = repo.read_changelog_bounded(3, 10).await.unwrap();
    assert_eq!((empty.bound, empty.confirmed), (2, true));
    assert!(empty.read.events.is_empty());
    assert_eq!(empty.read.gap_at, None);
    feed.shutdown();
}

// Baseline's read_from_bounded swallows this real range failure into a gap.
// Known holes/corruption remain gaps; only real store failures become Err.
#[tokio::test]
async fn real_range_failure_is_error_but_legacy_read_still_returns_gap() {
    let (repo, store, feed) = fixture().await;
    store.fail_read.store(true, Ordering::Release);
    let result = repo.read_changelog_bounded(7, 10).await;
    match result {
        Err(DbError::Storage(error)) => {
            assert_eq!(error, "changelog journal: injected journal range failure")
        }
        _ => panic!("real range failure must remain a storage error"),
    }
    let legacy = repo.read_changelog_from(7, 10).await.unwrap();
    assert!(legacy.events.is_empty());
    assert_eq!(legacy.gap_at, Some(7));
    store.fail_read.store(false, Ordering::Release);
    store
        .put(version_key(1), Bytes::from_static(b"invalid msgpack"))
        .await
        .unwrap();
    let corrupt = repo.read_changelog_bounded(1, 10).await.unwrap();
    assert!(corrupt.read.events.is_empty());
    assert_eq!(corrupt.read.gap_at, Some(1));
    feed.shutdown();
}
