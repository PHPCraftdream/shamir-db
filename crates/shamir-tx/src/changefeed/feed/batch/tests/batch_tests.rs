use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use bytes::Bytes;
use shamir_types::access::Actor;
use tokio::sync::Notify;

use super::super::{
    ChangelogError, ChangelogEvent, ChangelogStore, EventBatch, GapMarker, MAX_BYTES,
};

#[derive(Default)]
struct BatchStore {
    calls: Mutex<Vec<Vec<u64>>>,
    gap: Option<Arc<GapMarker>>,
    gaps_at_write: Mutex<Vec<Option<(u64, u64)>>>,
}

#[async_trait::async_trait]
impl ChangelogStore for BatchStore {
    async fn put(&self, _: Bytes, _: Bytes) -> Result<(), ChangelogError> {
        Ok(())
    }

    async fn put_many(&self, items: Vec<(Bytes, Bytes)>) -> Vec<Result<(), ChangelogError>> {
        if let Some(gap) = &self.gap {
            self.gaps_at_write
                .lock()
                .unwrap()
                .push(*gap.current.load_full());
        }
        self.calls.lock().unwrap().push(
            items
                .iter()
                .map(|(key, _)| u64::from_be_bytes(key.as_ref().try_into().unwrap()))
                .collect(),
        );
        items.into_iter().map(|_| Ok(())).collect()
    }

    async fn range_from(&self, _: Bytes, _: usize) -> Result<Vec<Bytes>, ChangelogError> {
        Ok(Vec::new())
    }
}

fn event(version: u64) -> Arc<ChangelogEvent> {
    Arc::new(ChangelogEvent {
        repo: "main".into(),
        commit_version: version,
        tx_id: version,
        actor: Actor::System,
        timestamp_ns: 0,
        changes: Vec::new(),
    })
}

fn marker() -> GapMarker {
    GapMarker {
        current: ArcSwap::from_pointee(None),
        persisted: ArcSwap::from_pointee(None),
        notify: Arc::new(Notify::new()),
    }
}

// The injected result enters the exact error path used by the production serializer.
#[tokio::test]
async fn serialization_error_flushes_prior_event_before_marking_gap() {
    let gap = Arc::new(marker());
    let fake = Arc::new(BatchStore {
        gap: Some(gap.clone()),
        ..Default::default()
    });
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let watermark = AtomicU64::new(0);
    let mut batch = EventBatch::default();
    batch
        .push_encoded(
            &store,
            event(1),
            Ok(Bytes::from_static(b"one")),
            &watermark,
            &gap,
        )
        .await;
    batch
        .push_encoded(
            &store,
            event(2),
            Err(ChangelogError::Serialize("injected".into())),
            &watermark,
            &gap,
        )
        .await;
    assert_eq!(*fake.calls.lock().unwrap(), vec![vec![1]]);
    assert_eq!(*fake.gaps_at_write.lock().unwrap(), vec![None]);
    assert_eq!(watermark.load(Ordering::Acquire), 1);
    assert_eq!(*gap.current.load_full(), Some((2, 2)));
    batch
        .push_encoded(
            &store,
            event(3),
            Ok(Bytes::from_static(b"three")),
            &watermark,
            &gap,
        )
        .await;
    batch.flush(&store, &watermark, &gap).await;
    assert_eq!(*fake.calls.lock().unwrap(), vec![vec![1], vec![3]]);
    assert_eq!(watermark.load(Ordering::Acquire), 3);
}

// Byte counts include each 8-byte key. The oversized entry must stand alone.
#[tokio::test]
async fn byte_budget_splits_and_persists_oversized_event_alone() {
    let fake = Arc::new(BatchStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let watermark = AtomicU64::new(0);
    let gap = marker();
    let mut batch = EventBatch::default();
    batch
        .push_encoded(
            &store,
            event(1),
            Ok(Bytes::from(vec![1; MAX_BYTES - 17])),
            &watermark,
            &gap,
        )
        .await;
    batch
        .push_encoded(
            &store,
            event(2),
            Ok(Bytes::from_static(b"xx")),
            &watermark,
            &gap,
        )
        .await;
    batch
        .push_encoded(
            &store,
            event(3),
            Ok(Bytes::from(vec![3; MAX_BYTES])),
            &watermark,
            &gap,
        )
        .await;
    batch
        .push_encoded(
            &store,
            event(4),
            Ok(Bytes::from_static(b"four")),
            &watermark,
            &gap,
        )
        .await;
    batch.flush(&store, &watermark, &gap).await;
    assert_eq!(
        *fake.calls.lock().unwrap(),
        vec![vec![1], vec![2], vec![3], vec![4]]
    );
    assert_eq!(watermark.load(Ordering::Acquire), 4);
    assert_eq!(*gap.current.load_full(), None);
}
