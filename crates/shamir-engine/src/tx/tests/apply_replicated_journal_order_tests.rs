//! S5b-4: follower journal publication precedes the live broadcast tail.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{ChangelogEvent, IsolationLevel, JournalRead, MvccStore, TxContext, TxId};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::broadcast::error::TryRecvError;

use crate::repo::repo_instance::ChangefeedHandle;
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::tests::journal_publish_order_tests::JournalPublishHook;
use crate::tx::{apply_replicated, ApplyOutcome, ApplyPolicy};

fn follower_repo() -> RepoInstance {
    RepoInstance::new(
        "follower".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("items")],
    )
}

fn put_event(value: &str, n: u8, leader_version: u64) -> ChangelogEvent {
    let mut bytes = [0u8; 16];
    bytes[15] = n;
    let record = RecordId(bytes);
    let mut tx = TxContext::new(TxId::new(leader_version), 0, 0, IsolationLevel::Snapshot);
    tx.set_actor(Actor::User(42));
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    tx.ensure_table_staging(table_token_for("items"), "items", store)
        .set(
            record.to_bytes().into(),
            InnerValue::Str(value.into()).to_bytes().unwrap(),
        );
    shamir_tx::project_event(&tx, "leader", leader_version).unwrap()
}

fn arm_broadcast(follower: &RepoInstance) -> Arc<JournalPublishHook> {
    let hook = Arc::new(JournalPublishHook::default());
    hook.broadcast_armed.store(true, Ordering::Release);
    follower.journal_publish_hook.store(Some(hook.clone()));
    hook
}

// S5b-2 is absent on this base: use the real completion/barrier/read primitives.
async fn bounded(follower: &RepoInstance, handle: &ChangefeedHandle, from: u64) -> JournalRead {
    let w = follower.tx_gate().await.unwrap().completion().watermark();
    let bound = handle
        .feed
        .confirm_through(w, Duration::from_secs(10))
        .await;
    assert_eq!(
        bound, w,
        "journal writer must confirm the completion prefix"
    );
    handle
        .feed
        .read_from_bounded(&handle.store, from, 17, bound)
        .await
}

fn applied(outcome: ApplyOutcome) -> u64 {
    match outcome {
        ApplyOutcome::Applied { local_version } => local_version,
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
async fn follower_journal_contains_a_and_b_while_a_broadcast_is_parked() {
    let follower = follower_repo();
    let gate = follower.tx_gate().await.unwrap();
    let handle = follower.changefeed().await.unwrap();
    let mut live = handle.feed.subscribe();
    let hook = arm_broadcast(&follower);
    let event_a = put_event("A", 1, 7);
    let event_b = put_event("B", 2, 9);
    let apply_a = apply_replicated(&follower, &event_a, &[], 0, ApplyPolicy::Trusted);
    tokio::pin!(apply_a);

    // Selection only BORROWS apply_a: losing the arm does not cancel the
    // partially applied operation; we retain it and drive it to completion.
    // Old follower never fires this hook: completion wins and fails an
    // assertion immediately, rather than awaiting an unreachable hook forever.
    let parked = tokio::select! {
        biased;
        _ = hook.reached.notified() => true,
        result = &mut apply_a => {
            assert!(result.is_ok(), "apply A unexpectedly failed: {result:?}");
            false
        },
    };
    assert!(parked, "apply A completed without the post-publish hook");
    let v_a = gate.completion().watermark();
    assert_eq!(v_a, 1);
    assert!(matches!(futures::poll!(&mut apply_a), Poll::Pending));
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));

    let v_b = applied(
        apply_replicated(&follower, &event_b, &[], 7, ApplyPolicy::Trusted)
            .await
            .unwrap(),
    );
    assert_eq!(v_b, v_a + 1);
    let read = bounded(&follower, &handle, v_a).await;
    // Capture the read while A is STILL parked; assertions follow release so
    // even an enqueue-after-publish mutation cannot strand a pending apply.
    assert!(matches!(futures::poll!(&mut apply_a), Poll::Pending));
    let live_b = live.try_recv().unwrap();
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
    hook.resume.notify_one();
    assert_eq!(applied(apply_a.await.unwrap()), v_a);
    let live_a = live.try_recv().unwrap();

    let mut expected_a = event_a.clone();
    expected_a.repo = "follower".into();
    expected_a.commit_version = v_a;
    let mut expected_b = event_b;
    expected_b.repo = "follower".into();
    expected_b.commit_version = v_b;
    assert_eq!(read.events, vec![expected_a.clone(), expected_b.clone()]);
    assert_eq!(read.gap_at, None);
    assert_eq!(*live_a, expected_a);
    assert_eq!(*live_b, expected_b);
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
}

/// SNAP-6: history fails after overlay publication. Keep both trackers pending
/// and emit neither a journal event nor a gap for this failed follower apply.
#[tokio::test]
async fn follower_failed_history_requires_reopen_without_journal_event_or_gap() {
    let follower = follower_repo();
    let _ = follower.get_table("items").await.unwrap();
    let gate = follower.tx_gate().await.unwrap();
    let handle = follower.changefeed().await.unwrap();
    let mut live = handle.feed.subscribe();
    let history: Arc<dyn Store> = Arc::new(FailingHistoryStore {
        inner: Arc::new(InMemoryStore::new()),
    });
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let token = table_token_for("items");
    assert!(follower.per_table_mvcc().remove_sync(&token).is_some());
    assert!(follower.per_table_mvcc().insert_sync(token, mvcc).is_ok());
    let event = put_event("history-fails", 3, 7);
    let error = apply_replicated(&follower, &event, &[], 0, ApplyPolicy::Trusted)
        .await
        .expect_err("history persistence must fail");
    assert!(
        matches!(&error, DbError::RepoRequiresReopen(_)),
        "{error:?}"
    );
    // History failure follows overlay publication, so this is not an abort.
    assert!(gate.requires_reopen());
    assert_eq!(gate.version_allocation_high_water_mark(), 1);
    assert_eq!(gate.completion().watermark(), 0);
    assert_eq!(gate.durable_watermark(), 0);
    let read = bounded(&follower, &handle, 1).await;
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, None);
    assert_eq!(handle.feed.first_gap_version(), 0);
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn follower_init_failure_gap_is_visible_before_broadcast_tail_resumes() {
    let follower = follower_repo();
    let gate = follower.tx_gate().await.unwrap();
    let hook = arm_broadcast(&follower);
    hook.init_failures.store(1, Ordering::Release);
    let event = put_event("init-fails", 4, 7);
    let apply = apply_replicated(&follower, &event, &[], 0, ApplyPolicy::Trusted);
    tokio::pin!(apply);
    // Borrow, never cancel, the apply future. Baseline's missing hook is a
    // deterministic assertion failure, not a hang; eventual-gap alone already
    // passes baseline because its legacy emit records a pending gap on error.
    let parked = tokio::select! {
        biased;
        _ = hook.reached.notified() => true,
        result = &mut apply => {
            assert!(result.is_ok(), "apply unexpectedly failed: {result:?}");
            false
        },
    };
    assert!(parked, "apply completed without the post-publish hook");
    let v = gate.completion().watermark();
    assert_eq!(v, 1);
    assert_eq!(hook.init_failures.load(Ordering::Acquire), 0);
    assert_eq!(gate.durable_watermark(), v);
    // Retry initialization BEFORE the tail resumes: the pending gap must
    // already exist and seed the new feed. No downstream event was enqueued.
    let handle = follower.changefeed().await.unwrap();
    let gap = handle.feed.first_gap_version();
    let read = bounded(&follower, &handle, v).await;
    assert!(matches!(futures::poll!(&mut apply), Poll::Pending));
    hook.resume.notify_one();
    assert_eq!(applied(apply.await.unwrap()), v);
    assert_eq!(gap, v);
    assert_eq!(read.gap_at, Some(v));
    assert!(read.events.is_empty());
}

type HistoryStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

// Same failed-history seam as the existing A12 tests, kept private here so
// their fixture and assertions remain unchanged.
struct FailingHistoryStore {
    inner: Arc<dyn Store>,
}

#[async_trait]
impl Store for FailingHistoryStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, _key: RecordKey, _value: Bytes) -> DbResult<bool> {
        Err(DbError::Storage("injected history failure".into()))
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    fn iter_stream(&self, batch_size: usize) -> HistoryStream {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> HistoryStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }

    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        batch_size: usize,
    ) -> HistoryStream {
        self.inner.iter_range_stream(start, end, batch_size)
    }

    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        batch_size: usize,
    ) -> HistoryStream {
        self.inner.iter_range_stream_reverse(start, end, batch_size)
    }
}
