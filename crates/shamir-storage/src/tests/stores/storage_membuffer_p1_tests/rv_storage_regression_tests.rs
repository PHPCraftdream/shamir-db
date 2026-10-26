use crate::error::{DbError, DbResult};
use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::{MemBufferConfig, MemBufferStore};
use crate::types::{KvOp, RecordKey, RecordStream, Repo, Store};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

fn config(max_entries: usize) -> MemBufferConfig {
    MemBufferConfig {
        max_bytes: 64 * 1024,
        max_entries,
        ttl_ms: None,
        flush_interval_ms: 600_000,
        flush_batch_size: 128,
    }
}

struct GateInner {
    inner: Arc<dyn Store>,
    pub entered: Mutex<Option<oneshot::Sender<()>>>,
    pub release: Mutex<Option<oneshot::Receiver<()>>>,
    pub gate: AtomicBool,
    pub fail_first: bool,
    pub fail_get: bool,
}
#[async_trait]
impl Store for GateInner {
    async fn insert(&self, v: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(v).await
    }
    async fn set(&self, k: RecordKey, v: Bytes) -> DbResult<bool> {
        if self.gate.swap(false, Ordering::AcqRel) {
            if let Some(tx) = self.entered.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let rx = { self.release.lock().unwrap().take() };
            if let Some(rx) = rx {
                let _ = rx.await;
                if self.fail_first {
                    return Err(DbError::Storage("injected set failure".into()));
                }
            }
        }
        self.inner.set(k, v).await
    }
    async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        if self.fail_get {
            return Err(DbError::Storage("injected get failure".into()));
        }
        self.inner.get(k).await
    }
    async fn remove(&self, k: RecordKey) -> DbResult<bool> {
        self.inner.remove(k).await
    }
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        let mut flags = Vec::with_capacity(items.len());
        for (key, value) in items {
            flags.push(self.set(key, value).await?);
        }
        Ok(flags)
    }
    async fn remove_many(&self, v: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.inner.remove_many(v).await
    }
    async fn transact(&self, v: Vec<KvOp>) -> DbResult<()> {
        self.inner.transact(v).await
    }
    fn iter_stream(&self, n: usize) -> RecordStream {
        self.inner.iter_stream(n)
    }
    fn scan_prefix_stream(&self, p: Bytes, n: usize) -> RecordStream {
        self.inner.scan_prefix_stream(p, n)
    }
    fn iter_range_stream(&self, a: Option<Bytes>, b: Option<Bytes>, n: usize) -> RecordStream {
        self.inner.iter_range_stream(a, b, n)
    }
    fn iter_range_stream_reverse(
        &self,
        a: Option<Bytes>,
        b: Option<Bytes>,
        n: usize,
    ) -> RecordStream {
        self.inner.iter_range_stream_reverse(a, b, n)
    }
}

#[tokio::test]
async fn rv_storage_c985620c_enforces_max_entries_without_disabling_cache() {
    let repo = InMemoryRepo::new();
    let inner = repo.store_get("cap").await.unwrap();
    let s = MemBufferStore::new(inner, config(1));
    s.disable_background_flusher_for_test();
    let a = s.insert(Bytes::from_static(b"a")).await.unwrap();
    let b = s.insert(Bytes::from_static(b"b")).await.unwrap();
    s.flush().await.unwrap();
    s.get(a).await.unwrap();
    s.get(b).await.unwrap();
    s.run_pending_cache_tasks().await;
    assert!(s.cache_bytes() > 0);
    assert!(s.cache_entry_count_for_test() <= 1);
    assert!(s.dirty_count_for_test() <= 1);
    assert!(s.cache_bytes() <= 64 * 1024);
    let z = MemBufferStore::new(s.inner().clone(), config(0));
    z.disable_background_flusher_for_test();
    let _ = z.get(RecordKey::from_slice(b"absent")).await;
    z.run_pending_cache_tasks().await;
    assert_eq!(z.cache_bytes(), 0);
}
#[tokio::test]
async fn rv_storage_eab39827_inner_only_remove_flags_are_exact() {
    let r = InMemoryRepo::new();
    let i = r.store_get("rm").await.unwrap();
    let k = RecordKey::from_slice(b"inner-only");
    i.set(k.clone(), Bytes::from_static(b"v")).await.unwrap();
    let s = MemBufferStore::new(i, config(10));
    s.disable_background_flusher_for_test();
    assert!(s.remove(k).await.unwrap());
    assert!(!s.remove(RecordKey::from_slice(b"missing")).await.unwrap());
}
#[tokio::test]
async fn rv_storage_eab39827_remove_many_inner_only_duplicates_report_once() {
    let repo = InMemoryRepo::new();
    let inner = repo.store_get("remove-many").await.unwrap();
    let key = RecordKey::from_slice(b"stored");
    inner
        .set(key.clone(), Bytes::from_static(b"v"))
        .await
        .unwrap();
    let store = MemBufferStore::new(inner, config(10));
    store.disable_background_flusher_for_test();
    assert_eq!(
        store
            .remove_many(vec![key.clone(), key.clone()])
            .await
            .unwrap(),
        vec![true, false]
    );
    assert_eq!(store.dirty_count_for_test(), 1);
}

#[tokio::test]
async fn rv_storage_adbb0c5b_existence_probe_error_does_not_publish_dirty() {
    let repo = InMemoryRepo::new();
    let backing = repo.store_get("get-fails").await.unwrap();
    let faulty = Arc::new(GateInner {
        inner: backing,
        entered: Mutex::new(None),
        release: Mutex::new(None),
        gate: AtomicBool::new(false),
        fail_first: false,
        fail_get: true,
    });
    let store = MemBufferStore::new(faulty, config(10));
    store.disable_background_flusher_for_test();
    assert!(store
        .set(RecordKey::from_slice(b"k"), Bytes::from_static(b"v"))
        .await
        .is_err());
    assert_eq!(store.dirty_count_for_test(), 0);
}

#[tokio::test]
async fn rv_storage_adbb0c5b_set_and_batch_flags_include_inner_and_duplicates() {
    let r = InMemoryRepo::new();
    let i = r.store_get("flags").await.unwrap();
    let k = RecordKey::from_slice(b"inner-only");
    i.set(k.clone(), Bytes::from_static(b"v")).await.unwrap();
    let s = MemBufferStore::new(i, config(10));
    s.disable_background_flusher_for_test();
    assert!(!s.set(k, Bytes::from_static(b"new")).await.unwrap());
    let x = RecordKey::from_slice(b"fresh");
    assert_eq!(
        s.set_many(vec![
            (x.clone(), Bytes::from_static(b"a")),
            (x, Bytes::from_static(b"b"))
        ])
        .await
        .unwrap(),
        vec![true, false]
    );
}
#[tokio::test]
async fn rv_storage_491185fb_transact_only_cleans_touched_dirty_keys() {
    let r = InMemoryRepo::new();
    let i = r.store_get("touched").await.unwrap();
    let s = MemBufferStore::new(i.clone(), config(10));
    s.disable_background_flusher_for_test();
    let u = RecordKey::from_slice(b"untouched");
    let t = RecordKey::from_slice(b"touched");
    s.set(u.clone(), Bytes::from_static(b"u")).await.unwrap();
    s.set(t.clone(), Bytes::from_static(b"old")).await.unwrap();
    s.transact(vec![KvOp::Set(t, Bytes::from_static(b"new"))])
        .await
        .unwrap();
    assert!(i.get(u).await.is_err());
    assert_eq!(s.dirty_count_for_test(), 1);
}
#[tokio::test]
async fn rv_storage_e47dbf27_serializes_concurrent_drain_submission() {
    let r = InMemoryRepo::new();
    let backing = r.store_get("order").await.unwrap();
    let (tx, rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(GateInner {
        inner: backing.clone(),
        entered: Mutex::new(Some(tx)),
        release: Mutex::new(Some(release_rx)),
        gate: AtomicBool::new(true),
        fail_first: false,
        fail_get: false,
    });
    let s = Arc::new(MemBufferStore::new(gate, config(10)));
    s.disable_background_flusher_for_test();
    let k = RecordKey::from_slice(b"k");
    s.set(k.clone(), Bytes::from_static(b"old")).await.unwrap();
    let d = {
        let s = s.clone();
        tokio::spawn(async move { s.drain_once_for_test(1).await })
    };
    rx.await.unwrap();
    let writer = {
        let s = s.clone();
        let k = k.clone();
        tokio::spawn(async move { s.set(k, Bytes::from_static(b"new")).await })
    };
    writer.await.unwrap().unwrap();
    let d2 = s.drain_once_for_test(1);
    tokio::pin!(d2);
    assert!(
        futures::poll!(d2.as_mut()).is_pending(),
        "second drain waits for the active backing submission"
    );
    release_tx.send(()).unwrap();
    assert_eq!(d.await.unwrap().unwrap(), 1);
    assert_eq!(
        backing.get(k.clone()).await.unwrap(),
        Bytes::from_static(b"old")
    );
    assert_eq!(d2.await.unwrap(), 1);
    s.flush().await.unwrap();
    s.evict_from_cache_for_test(&k).await;
    assert_eq!(
        backing.get(k.clone()).await.unwrap(),
        Bytes::from_static(b"new")
    );
    assert_eq!(s.get(k).await.unwrap(), Bytes::from_static(b"new"));
}
#[tokio::test]
async fn rv_storage_e47dbf27_transact_waits_for_inflight_drain() {
    let repo = InMemoryRepo::new();
    let backing = repo.store_get("txn-order").await.unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let inner = Arc::new(GateInner {
        inner: backing.clone(),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        gate: AtomicBool::new(true),
        fail_first: false,
        fail_get: false,
    });
    let store = Arc::new(MemBufferStore::new(inner, config(10)));
    store.disable_background_flusher_for_test();
    let key = RecordKey::from_slice(b"txn-order-key");
    store
        .set(key.clone(), Bytes::from_static(b"old"))
        .await
        .unwrap();
    let drain = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.drain_once_for_test(1).await })
    };
    entered_rx.await.unwrap();
    let transaction =
        store.transact_once_for_test(vec![KvOp::Set(key.clone(), Bytes::from_static(b"new"))]);
    tokio::pin!(transaction);
    assert!(futures::poll!(transaction.as_mut()).is_pending());
    release_tx.send(()).unwrap();
    drain.await.unwrap().unwrap();
    transaction.await.unwrap();
    assert_eq!(backing.get(key).await.unwrap(), Bytes::from_static(b"new"));
}

#[tokio::test]
async fn rv_storage_707bd4fd_drop_reports_dirty_loss() {
    let r = InMemoryRepo::new();
    let i = r.store_get("drop").await.unwrap();
    let s = MemBufferStore::new(i, config(10));
    s.disable_background_flusher_for_test();
    let telemetry = s.dirty_drop_telemetry_for_test();
    s.set(RecordKey::from_slice(b"lost"), Bytes::from_static(b"v"))
        .await
        .unwrap();
    drop(s);
    assert_eq!(telemetry.load(Ordering::Acquire), 1);
}
#[tokio::test]
async fn rv_storage_995fd07d_exposes_flush_error_counter() {
    let r = InMemoryRepo::new();
    let inner = r.store_get("errors").await.unwrap();
    let backing = inner.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let failing = Arc::new(GateInner {
        inner,
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        gate: AtomicBool::new(true),
        fail_first: true,
        fail_get: false,
    });
    let s = MemBufferStore::new(failing, config(10));
    s.set(RecordKey::from_slice(b"k"), Bytes::from_static(b"v"))
        .await
        .unwrap();
    s.wake_flusher_for_test();
    entered_rx.await.unwrap();
    release_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while s.flush_errors() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(s.dirty_count_for_test() > 0);
    assert!(backing.get(RecordKey::from_slice(b"k")).await.is_err());
    s.shutdown().await.unwrap();
    assert_eq!(
        backing.get(RecordKey::from_slice(b"k")).await.unwrap(),
        Bytes::from_static(b"v")
    );
}
