//! SNAP-5: acknowledgement follows the contiguous completion prefix.

use std::future::Future;
use std::sync::Arc;
use std::task::Poll;

use bytes::Bytes;
use futures::future::poll_fn;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::RecordKey;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;

#[derive(Clone, Copy, Debug)]
enum Write {
    Set,
    Many,
    AppendOnly,
    Delete,
}

impl Write {
    async fn apply(self, mvcc: &MvccStore) -> DbResult<u64> {
        let key = RecordKey::from_slice(b"key");
        let value = Bytes::from_static(b"written");
        match self {
            Self::Set => mvcc.set_versioned(key, value).await,
            Self::Many => {
                mvcc.set_versioned_many(vec![
                    (key, value),
                    (
                        RecordKey::from_slice(b"other"),
                        Bytes::from_static(b"other"),
                    ),
                ])
                .await
            }
            Self::AppendOnly => {
                mvcc.set_versioned_many_append_only(vec![
                    (key, value),
                    (
                        RecordKey::from_slice(b"other"),
                        Bytes::from_static(b"other"),
                    ),
                ])
                .await
            }
            Self::Delete => mvcc.delete_versioned(key).await,
        }
    }

    fn expected(self) -> Option<Bytes> {
        match self {
            Self::Delete => None,
            Self::Set | Self::Many | Self::AppendOnly => Some(Bytes::from_static(b"written")),
        }
    }
}

fn fixture() -> (Arc<RepoTxGate>, Arc<MvccStore>) {
    let gate = Arc::new(RepoTxGate::new(0, 1));
    let mvcc = Arc::new(MvccStore::new(
        Arc::new(InMemoryStore::new()),
        Arc::clone(&gate),
    ));
    mvcc.set_test_now(1_000);
    (gate, mvcc)
}

// The handshake observes a poll AFTER the durable write and vacuum, not a
// guessed scheduling delay. Red without the ack wait: that poll is Ready.
// Also reused by the GC fixtures that must inspect writes while W is parked.
pub(crate) async fn spawn_pending_write<F>(
    gate: Arc<RepoTxGate>,
    target: u64,
    write: F,
) -> JoinHandle<DbResult<u64>>
where
    F: Future<Output = DbResult<u64>> + Send + 'static,
{
    let registrations = gate.completion().waiter_registrations();
    let (sender, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut write = Box::pin(write);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = write.as_mut().poll(cx);
            if gate.completion().waiter_registrations() > registrations || result.is_ready() {
                if let Some(sender) = sender.take() {
                    sender.send(result.is_pending()).unwrap();
                }
            }
            result
        })
        .await
    });
    assert!(receiver.await.unwrap(), "ack for {target} must be pending");
    task
}

async fn parked_lower(abort: bool) {
    for write in [Write::Set, Write::Many, Write::AppendOnly, Write::Delete] {
        let (gate, mvcc) = fixture();
        if matches!(write, Write::Delete) {
            mvcc.set_versioned(RecordKey::from_slice(b"key"), Bytes::from_static(b"old"))
                .await
                .unwrap();
        }
        let lower = gate.assign_next_version_guarded();
        let floor = gate.visible_floor();
        let width = if matches!(write, Write::Many | Write::AppendOnly) {
            2
        } else {
            1
        };
        let target = lower.version() + width;
        let owned = Arc::clone(&mvcc);
        let task = spawn_pending_write(Arc::clone(&gate), target, async move {
            write.apply(&owned).await
        })
        .await;
        assert_eq!(gate.visible_floor(), floor);
        assert_eq!(gate.completion().waiter_count(), 1);
        assert!(!task.is_finished());
        if abort {
            drop(lower);
        } else {
            let version = lower.version();
            lower.commit();
            gate.mark_durable(version);
        }
        assert_eq!(task.await.unwrap().unwrap(), target);
        assert_eq!(gate.visible_floor(), target);
        assert_eq!(gate.durable_watermark(), target);
        let snapshot = gate.open_snapshot().await;
        assert_eq!(snapshot.version(), target);
        assert_eq!(
            mvcc.get_at(b"key", snapshot.version()).await.unwrap(),
            write.expected(),
        );
        if matches!(write, Write::Many | Write::AppendOnly) {
            assert_eq!(
                mvcc.get_at(b"other", snapshot.version()).await.unwrap(),
                Some(Bytes::from_static(b"other")),
            );
        }
    }
}

#[tokio::test]
async fn non_tx_ack_waits_for_lower_completion_and_snapshot_sees_write() {
    parked_lower(false).await;
}

#[tokio::test]
async fn non_tx_ack_waits_for_lower_abort_and_snapshot_sees_write() {
    parked_lower(true).await;
}

#[tokio::test]
async fn non_tx_already_visible_ack_does_not_suspend_or_register() {
    for write in [Write::Set, Write::Many, Write::AppendOnly, Write::Delete] {
        let (gate, mvcc) = fixture();
        let registrations = gate.completion().waiter_registrations();
        let mut operation = Box::pin(write.apply(&mvcc));
        let result = poll_fn(|cx| Poll::Ready(operation.as_mut().poll(cx))).await;
        let Poll::Ready(result) = result else {
            panic!("uncontended in-memory write must finish in one poll");
        };
        let version = result.unwrap();
        assert_eq!(gate.visible_floor(), version);
        assert_eq!(gate.completion().waiter_registrations(), registrations);
        let snapshot = gate.open_snapshot().await;
        assert_eq!(
            mvcc.get_at(b"key", snapshot.version()).await.unwrap(),
            write.expected(),
        );
    }
}

#[tokio::test]
async fn non_tx_reopen_required_returns_typed_committed_exception() {
    for write in [Write::Set, Write::Many, Write::AppendOnly, Write::Delete] {
        let (gate, mvcc) = fixture();
        let lower = gate.assign_next_version_guarded();
        gate.set_requires_reopen();
        let error = write.apply(&mvcc).await.unwrap_err();
        let shamir_storage::error::DbError::WriteCommittedVisibilityPending {
            commit_version: version,
            visible_through,
            reason,
        } = error
        else {
            panic!("expected committed visibility exception, got {error:?}");
        };
        assert_eq!(reason, "reopen_required");
        assert_eq!(visible_through, 0);
        assert!(version > lower.version());
        assert_eq!(gate.visible_floor(), 0);
        assert_eq!(
            mvcc.get_at(b"key", version).await.unwrap(),
            write.expected()
        );
        drop(lower);
        assert_eq!(gate.visible_floor(), version);
    }
}

#[tokio::test]
async fn guarded_no_wait_variants_do_not_block_lower_orphan_replay() {
    for write in [Write::Set, Write::Many, Write::AppendOnly, Write::Delete] {
        let (gate, mvcc) = fixture();
        let lower = gate.assign_next_version_guarded();
        let key = RecordKey::from_slice(b"key");
        let value = Bytes::from_static(b"written");
        let version = match write {
            Write::Set => mvcc.set_versioned_no_wait(key, value).await,
            Write::Many => mvcc.set_versioned_many_no_wait(vec![(key, value)]).await,
            Write::AppendOnly => {
                mvcc.set_versioned_many_append_only_no_wait(vec![(key, value)])
                    .await
            }
            Write::Delete => mvcc.delete_versioned_no_wait(key).await,
        }
        .unwrap();
        assert_eq!(gate.visible_floor(), 0);
        assert_eq!(gate.completion().waiter_registrations(), 0);
        assert_eq!(
            mvcc.get_at(b"key", version).await.unwrap(),
            write.expected()
        );
        drop(lower);
        assert_eq!(gate.visible_floor(), version);
        assert_eq!(gate.durable_watermark(), version);
    }
}

#[tokio::test]
async fn cancelling_non_tx_ack_preserves_commit_and_releases_waiter() {
    let (gate, mvcc) = fixture();
    let lower = gate.assign_next_version_guarded();
    let owned = Arc::clone(&mvcc);
    let task = spawn_pending_write(Arc::clone(&gate), 2, async move {
        Write::Set.apply(&owned).await
    })
    .await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(gate.completion().waiter_count(), 0);
    assert_eq!(gate.version_allocation_high_water_mark(), 2);
    assert_eq!(mvcc.get_at(b"key", 2).await.unwrap(), Write::Set.expected());
    drop(lower);
    assert_eq!(gate.visible_floor(), 2);
    assert_eq!(gate.durable_watermark(), 2);
    let snapshot = gate.open_snapshot().await;
    assert_eq!(
        mvcc.get_at(b"key", snapshot.version()).await.unwrap(),
        Write::Set.expected(),
    );
}
