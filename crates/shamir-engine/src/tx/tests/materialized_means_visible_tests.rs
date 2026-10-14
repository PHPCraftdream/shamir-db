use std::future::{pending, poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{stream, Stream};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{CommitVisibility, IndexWriteOp, IsolationLevel, MvccStore, TxContext, TxId};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::{broadcast::error::TryRecvError, oneshot, Notify};
use tokio::task::JoinHandle;

use crate::repo::RepoInstance;
use crate::table::table_manager::table_token_for;
use crate::tx::commit::TxError;
use crate::tx::commit_phases::{
    FAIL_PHASE_5A_TABLE_TOKEN, FAIL_PHASE_5A_TX_ID, FAIL_PHASE_5C_TX_ID,
};
use crate::tx::tests::post_wal_orphan_ordering_tests::fixture;
use crate::tx::{apply_replicated, ApplyOutcome, ApplyPolicy, MaterializationState, TxOutcome};

// Shared by older deferral regressions: observe an actual pending driver poll,
// never inline-await an ack whose orphan replay is deliberately suppressed.
pub(super) async fn spawn_deferred_commit(
    repo: &RepoInstance,
    tx: TxContext,
) -> JoinHandle<Result<TxOutcome, TxError>> {
    let gate = repo.tx_gate().await.unwrap();
    let version = gate.version_allocation_high_water_mark() + 1;
    let owned = repo.clone();
    let (sender, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let commit = owned.commit_tx(tx);
        tokio::pin!(commit);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = commit.as_mut().poll(cx);
            if gate.is_orphaned_post_wal(version) || result.is_ready() {
                if let Some(sender) = sender.take() {
                    sender.send(result.is_pending()).unwrap();
                }
            }
            result
        })
        .await
    });
    assert!(receiver.await.unwrap(), "deferred ack must wait for replay");
    task
}

// Red before SNAP-6: unconditional settlement acknowledges a missing row and
// broadcasts an event for it. Poll the actual commit to prove the ack is parked,
// rather than infer progress from a yield or wall-clock delay.
async fn failed_projection_waits_for_replay(visibility: CommitVisibility, index_failure: bool) {
    let (repo, _) = fixture("materialized_means_visible").await;
    let table = repo.get_table("a").await.unwrap();
    let healthy = repo.get_table("b").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let feed = repo.changefeed().await.unwrap();
    let mut live = feed.feed.subscribe();
    let (mut tx, writer_snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tx.set_visibility(visibility);
    let value = InnerValue::Str("deferred".into());
    let id = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    let healthy_id = healthy.insert_tx(&value, Some(&mut tx)).await.unwrap();
    let posting_key = Bytes::from_static(b"snap6-posting");
    let posting_value = Bytes::from_static(b"snap6-visible-posting");
    if index_failure {
        tx.index_write_set.push((
            table_token_for("a"),
            IndexWriteOp::SetPosting {
                key: posting_key.clone(),
                value: posting_value.clone(),
                provenance: Default::default(),
            },
        ));
        FAIL_PHASE_5C_TX_ID.store(tx.tx_id.0, Ordering::SeqCst);
    } else {
        FAIL_PHASE_5A_TABLE_TOKEN.store(table_token_for("a"), Ordering::SeqCst);
        FAIL_PHASE_5A_TX_ID.store(tx.tx_id.0, Ordering::SeqCst);
    }
    let version = gate.version_allocation_high_water_mark() + 1;
    let owned = repo.clone();
    let observer = Arc::clone(&gate);
    let (sender, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let commit = owned.commit_tx(tx);
        tokio::pin!(commit);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = commit.as_mut().poll(cx);
            if observer.is_orphaned_post_wal(version) || result.is_ready() {
                if let Some(sender) = sender.take() {
                    sender.send(result.is_pending()).unwrap();
                }
            }
            result
        })
        .await
    });
    let pending = receiver.await.unwrap();
    FAIL_PHASE_5A_TX_ID.store(0, Ordering::SeqCst);
    FAIL_PHASE_5A_TABLE_TOKEN.store(0, Ordering::SeqCst);
    FAIL_PHASE_5C_TX_ID.store(0, Ordering::SeqCst);
    assert!(pending, "ack must wait for orphan replay");
    assert!(gate.visible_floor() < version);
    assert!(gate.is_orphaned_post_wal(version));
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
    let (old, old_guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert!(old.snapshot_version < version);
    assert!(matches!(
        table.read_one_tx(id, Some(&old)).await,
        Err(DbError::NotFound(_))
    ));
    assert!(matches!(
        healthy.read_one_tx(healthy_id, Some(&old)).await,
        Err(DbError::NotFound(_))
    ));
    assert_eq!(healthy.get(healthy_id).await.unwrap(), value);
    if index_failure {
        assert!(matches!(
            table.info_store().get(posting_key.clone().into()).await,
            Err(DbError::NotFound(_))
        ));
        assert_eq!(table.get(id).await.unwrap(), value);
    }
    let replayed_entry = repo.drainer().window_entry(version).unwrap();
    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 1);
    let outcome = task.await.unwrap().unwrap();
    assert_eq!(outcome.commit_version, version);
    assert_eq!(outcome.materialization, MaterializationState::Deferred);
    if let Some(background) = outcome.background {
        assert_eq!(background.join().await, MaterializationState::Deferred);
    }
    assert!(gate.visible_floor() >= version);
    assert!(!gate.is_orphaned_post_wal(version));
    if index_failure {
        assert_eq!(
            table.info_store().get(posting_key.into()).await.unwrap(),
            posting_value,
        );
    }
    assert!(matches!(
        table.read_one_tx(id, Some(&old)).await,
        Err(DbError::NotFound(_))
    ));
    let (new, new_guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert_eq!(table.read_one_tx(id, Some(&new)).await.unwrap(), value);
    assert_eq!(
        healthy.read_one_tx(healthy_id, Some(&new)).await.unwrap(),
        value,
    );
    assert!(matches!(
        healthy.read_one_tx(healthy_id, Some(&old)).await,
        Err(DbError::NotFound(_))
    ));
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
    // A derived WAL event repairs the orphan while preserving visibility guarantees.
    let (events, gap) = repo
        .read_changelog_for_replication(version, 1)
        .await
        .unwrap();
    assert_eq!(gap, None);
    assert_eq!(events.len(), 1);
    let entry = repo.drainer().window_entry(version);
    assert!(entry.is_none());
    assert_eq!(events[0].event.commit_version, version);
    assert_eq!(
        events[0].event,
        shamir_tx::event_from_wal_entry(&replayed_entry).unwrap()
    );
    drop((writer_snapshot, old_guard, new_guard));
}

#[tokio::test]
async fn sync_failed_data_stalls_visibility_and_preserves_old_snapshot() {
    failed_projection_waits_for_replay(CommitVisibility::Synchronous, false).await;
}

#[tokio::test]
async fn async_index_failed_data_stalls_visibility_and_preserves_old_snapshot() {
    failed_projection_waits_for_replay(CommitVisibility::AsyncIndex, false).await;
}

// Red before SNAP-6: failed postings still let W pass the data version.
#[tokio::test]
async fn sync_failed_index_stalls_visibility_until_posting_replay() {
    failed_projection_waits_for_replay(CommitVisibility::Synchronous, true).await;
}

#[tokio::test]
async fn successful_data_still_settles_and_emits_visible_event() {
    let (repo, _) = fixture("materialized_success").await;
    let table = repo.get_table("a").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let mut live = repo.subscribe_changelog().await.unwrap();
    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let value = InnerValue::Str("visible".into());
    let id = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    let outcome = repo.commit_tx(tx).await.unwrap();
    assert_eq!(outcome.materialization, MaterializationState::Complete);
    assert_eq!(gate.visible_floor(), outcome.commit_version);
    assert!(!gate.is_orphaned_post_wal(outcome.commit_version));
    assert_eq!(
        live.try_recv().unwrap().commit_version,
        outcome.commit_version,
    );
    let (read, read_guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert_eq!(table.read_one_tx(id, Some(&read)).await.unwrap(), value);
    drop((guard, read_guard));
}

// A history failure is NOT a pure failure: apply_committed_ops first publishes
// the overlay. A pre-image failure, on the other hand, has published no rows.
async fn follower_failure(pre_image: bool, two_tables: bool) {
    let (repo, _) = fixture("follower_materialized").await;
    let gate = repo.tx_gate().await.unwrap();
    let mut tx = TxContext::new(TxId::new(19), 0, 0, IsolationLevel::Snapshot);
    let id = RecordId::new();
    let value = InnerValue::Str("replicated".into());
    let names = if two_tables {
        vec!["a", "b"]
    } else {
        vec!["a"]
    };
    for name in &names {
        repo.get_table(name).await.unwrap();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        tx.ensure_table_staging(table_token_for(name), name, store)
            .set(id.to_bytes().into(), value.to_bytes().unwrap());
    }
    let event = shamir_tx::project_event(&tx, "leader", 19).unwrap();
    // Reproduce the grouping order, not an assumed alphabetical table order.
    let mut order = shamir_collections::TFxMap::default();
    for change in &event.changes {
        order.entry(change.table.clone()).or_insert(());
    }
    let target = order.keys().last().unwrap();
    let token = table_token_for(target);
    let history: Arc<dyn Store> = Arc::new(FailingStore {
        inner: Arc::new(InMemoryStore::new()),
        pre_image,
        park: None,
    });
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    repo.per_table_mvcc().remove_sync(&token).unwrap();
    assert!(repo
        .per_table_mvcc()
        .insert_sync(token, Arc::clone(&mvcc))
        .is_ok());
    let mut live = repo.subscribe_changelog().await.unwrap();
    let error = apply_replicated(&repo, &event, &[], 0, ApplyPolicy::Trusted)
        .await
        .unwrap_err();
    let version = gate.version_allocation_high_water_mark();
    let partial = two_tables || !pre_image;
    // A partial apply is terminal (reopen required); an untouched one keeps the raw error.
    if partial {
        assert!(matches!(error, DbError::RepoRequiresReopen(_)), "{error:?}");
    } else {
        assert!(matches!(error, DbError::Storage(_)), "{error:?}");
    }
    assert_eq!(gate.requires_reopen(), partial);
    assert_eq!(
        gate.visible_floor(),
        if partial { version - 1 } else { version },
    );
    assert_eq!(
        gate.durable_watermark(),
        if partial { version - 1 } else { version },
    );
    assert!(!gate.is_orphaned_post_wal(version));
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
    if !pre_image {
        assert_eq!(mvcc.version_of(id.as_bytes()), version);
    }
    if partial {
        assert_eq!(
            gate.await_visible(version).await,
            shamir_tx::repo_tx_gate::VisibleOutcome::ReopenRequired,
        );
    } else {
        // Pure failure does not clog a subsequent successful follower apply.
        repo.per_table_mvcc().remove_sync(&token).unwrap();
        let healthy = Arc::new(MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::clone(&gate),
        ));
        assert!(repo.per_table_mvcc().insert_sync(token, healthy).is_ok());
        let applied = apply_replicated(&repo, &event, &[], 0, ApplyPolicy::Trusted)
            .await
            .unwrap();
        assert!(matches!(applied, ApplyOutcome::Applied { .. }));
        assert_eq!(gate.visible_floor(), version + 1);
        assert_eq!(live.try_recv().unwrap().commit_version, version + 1);
    }
}

// Red before SNAP-6: history Err was silently marked Aborted despite overlay rows.
#[tokio::test]
async fn follower_history_failure_requires_reopen_without_settling_visible_rows() {
    follower_failure(false, false).await;
}

// Red before SNAP-6: a later pre-image failure aborted an earlier visible table.
#[tokio::test]
async fn follower_second_table_failure_keeps_partial_version_unsettled() {
    follower_failure(true, true).await;
}

#[tokio::test]
async fn follower_pure_pre_image_failure_aborts_without_clogging_visibility() {
    follower_failure(true, false).await;
}

// Red before the pre-visibility guard transition: cancellation during history
// or a later table's pre-image read silently aborted an already visible version.
async fn follower_cancel(pre_image: bool, two_tables: bool) {
    let (repo, _) = fixture("follower_cancel_visibility").await;
    let gate = repo.tx_gate().await.unwrap();
    let id = RecordId::new();
    let value = InnerValue::Str("cancelled-apply".into());
    let mut tx = TxContext::new(TxId::new(23), 0, 0, IsolationLevel::Snapshot);
    let names = if two_tables {
        vec!["a", "b"]
    } else {
        vec!["a"]
    };
    for name in names {
        repo.get_table(name).await.unwrap();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        tx.ensure_table_staging(table_token_for(name), name, store)
            .set(id.to_bytes().into(), value.to_bytes().unwrap());
    }
    let event = shamir_tx::project_event(&tx, "leader", 23).unwrap();
    let mut order = shamir_collections::TFxMap::default();
    for change in &event.changes {
        order.entry(change.table.clone()).or_insert(());
    }
    let first_token = table_token_for(order.keys().next().unwrap());
    let target_token = table_token_for(order.keys().last().unwrap());
    let reached = Arc::new(Notify::new());
    let history: Arc<dyn Store> = Arc::new(FailingStore {
        inner: Arc::new(InMemoryStore::new()),
        pre_image: false,
        park: Some((Arc::clone(&reached), pre_image)),
    });
    let target = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    repo.per_table_mvcc().remove_sync(&target_token).unwrap();
    assert!(repo
        .per_table_mvcc()
        .insert_sync(target_token, Arc::clone(&target))
        .is_ok());
    let first = repo
        .per_table_mvcc()
        .read_sync(&first_token, |_, mvcc| Arc::clone(mvcc))
        .unwrap();
    let mut live = repo.subscribe_changelog().await.unwrap();
    let owned = repo.clone();
    let task = tokio::spawn(async move {
        apply_replicated(&owned, &event, &[], 0, ApplyPolicy::Trusted).await
    });
    reached.notified().await;
    let version = gate.version_allocation_high_water_mark();
    let partial = two_tables || !pre_image;
    assert_eq!(
        first.version_of(id.as_bytes()),
        if partial { version } else { 0 },
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(gate.requires_reopen(), partial);
    assert_eq!(
        gate.visible_floor(),
        if partial { version - 1 } else { version },
    );
    assert_eq!(
        gate.durable_watermark(),
        if partial { version - 1 } else { version },
    );
    assert!(!gate.is_orphaned_post_wal(version));
    assert!(matches!(live.try_recv(), Err(TryRecvError::Empty)));
    if partial {
        assert_eq!(
            first.get_current_many(&[id.to_bytes()]).await.unwrap(),
            vec![Some(value.to_bytes().unwrap())],
        );
        assert_eq!(
            first
                .get_at(id.as_bytes(), gate.visible_floor())
                .await
                .unwrap(),
            None,
        );
    }
}

#[tokio::test]
async fn follower_cancel_during_history_keeps_visible_version_unsettled() {
    follower_cancel(false, false).await;
}

#[tokio::test]
async fn follower_cancel_during_next_pre_image_keeps_partial_version_unsettled() {
    follower_cancel(true, true).await;
}

// Safety control: pre-image cancellation before any apply remains an A12 abort.
#[tokio::test]
async fn follower_cancel_before_visibility_aborts_without_reopen() {
    follower_cancel(true, false).await;
}

type HistoryStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

struct FailingStore {
    inner: Arc<dyn Store>,
    pre_image: bool,
    // Some parks at history set (false) or at pre-image range scan (true).
    park: Option<(Arc<Notify>, bool)>,
}

#[async_trait]
impl Store for FailingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, _key: RecordKey, _value: Bytes) -> DbResult<bool> {
        if let Some((reached, false)) = &self.park {
            reached.notify_one();
            pending::<()>().await;
        }
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
        if let Some((reached, true)) = &self.park {
            let reached = Arc::clone(reached);
            return Box::pin(stream::once(async move {
                reached.notify_one();
                pending::<DbResult<Vec<(RecordKey, Bytes)>>>().await
            }));
        }
        if self.pre_image {
            return Box::pin(stream::once(async {
                Err(DbError::Storage("injected pre-image failure".into()))
            }));
        }
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
