use crate::error::DbResult;
use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
use crate::types::{KvOp, RecordKey, RecordStream, Repo, Store};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::oneshot;

fn config() -> MemBufferConfig {
    MemBufferConfig {
        max_bytes: 65536,
        max_entries: 16,
        ttl_ms: None,
        flush_interval_ms: 600_000,
        flush_batch_size: 16,
    }
}

struct CallbackInner {
    backing: Arc<dyn Store>,
    buffered: Mutex<Option<Weak<MemBufferStore>>>,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    callback_key: RecordKey,
}

#[async_trait]
impl Store for CallbackInner {
    async fn insert(&self, v: Bytes) -> DbResult<RecordKey> {
        self.backing.insert(v).await
    }
    async fn set(&self, k: RecordKey, v: Bytes) -> DbResult<bool> {
        self.backing.set(k, v).await
    }
    async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        self.backing.get(k).await
    }
    async fn remove(&self, k: RecordKey) -> DbResult<bool> {
        self.backing.remove(k).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        if let Some(tx) = self.entered.lock().unwrap().take() {
            let _ = tx.send(());
        }
        let release = { self.release.lock().unwrap().take() };
        if let Some(rx) = release {
            let _ = rx.await;
        }
        let buffered = self
            .buffered
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
            .unwrap();
        buffered
            .set(self.callback_key.clone(), Bytes::from_static(b"old"))
            .await?;
        self.backing.transact(ops).await
    }
    fn iter_stream(&self, n: usize) -> RecordStream {
        self.backing.iter_stream(n)
    }
    fn scan_prefix_stream(&self, p: Bytes, n: usize) -> RecordStream {
        self.backing.scan_prefix_stream(p, n)
    }
    fn iter_range_stream(&self, a: Option<Bytes>, b: Option<Bytes>, n: usize) -> RecordStream {
        self.backing.iter_range_stream(a, b, n)
    }
    fn iter_range_stream_reverse(
        &self,
        a: Option<Bytes>,
        b: Option<Bytes>,
        n: usize,
    ) -> RecordStream {
        self.backing.iter_range_stream_reverse(a, b, n)
    }
}

#[tokio::test]
async fn rv_storage_e47dbf27_transact_callback_preserves_replacement_and_serializes_drain() {
    let repo = InMemoryRepo::new();
    let backing = repo.store_get("transaction-callback").await.unwrap();
    let key = RecordKey::from_slice(b"callback-key");
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let inner = Arc::new(CallbackInner {
        backing: backing.clone(),
        buffered: Mutex::new(None),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        callback_key: key.clone(),
    });
    let store = Arc::new(MemBufferStore::new(inner.clone(), config()));
    store.disable_background_flusher_for_test();
    *inner.buffered.lock().unwrap() = Some(Arc::downgrade(&store));
    store
        .set(key.clone(), Bytes::from_static(b"old"))
        .await
        .unwrap();
    let txn = {
        let store = store.clone();
        let key = key.clone();
        tokio::spawn(async move {
            store
                .transact(vec![KvOp::Set(key, Bytes::from_static(b"new"))])
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    let drain = store.drain_once_for_test(1);
    tokio::pin!(drain);
    assert!(futures::poll!(drain.as_mut()).is_pending());
    release_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        txn.await.unwrap().unwrap();
        drain.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(
        backing.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"old")
    );
    assert_eq!(store.get(key).await.unwrap(), Bytes::from_static(b"old"));
    assert!(store.is_dirty_empty());
}

#[tokio::test]
async fn rv_storage_eab39827_remove_many_wakes_flush_notification() {
    let repo = InMemoryRepo::new();
    let backing = repo.store_get("remove-notification").await.unwrap();
    let store = MemBufferStore::new(backing, config());
    store.disable_background_flusher_for_test();
    let key = RecordKey::from_slice(b"removed");
    store.remove_many(vec![key.clone()]).await.unwrap();
    let notified = store.wait_flush_notification_for_test();
    tokio::pin!(notified);
    assert!(futures::poll!(notified.as_mut()).is_ready());
    assert!(store.get(key).await.is_err());
}
