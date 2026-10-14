use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;

use super::super::barrier::{verify_events_and_flush, verify_events_without_flush};
use super::super::in_flight_range;
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_replication_bookmark, save_bookmark_and_clean_horizon,
};

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

#[derive(Default)]
struct DiskImage {
    pending: Mutex<BTreeMap<RecordKey, Bytes>>,
    durable: Mutex<BTreeMap<RecordKey, Bytes>>,
    fail_flush: AtomicBool,
}

impl DiskImage {
    fn crash_image(&self) -> Arc<dyn Store> {
        Arc::new(SnapshotStore {
            records: Mutex::new(self.durable.lock().unwrap().clone()),
        })
    }
}

struct SnapshotStore {
    records: Mutex<BTreeMap<RecordKey, Bytes>>,
}

#[async_trait]
impl Store for SnapshotStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let key = RecordKey::from_slice(&(self.records.lock().unwrap().len() as u64).to_be_bytes());
        self.set(key, value).await?;
        Ok(key)
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        Ok(self.records.lock().unwrap().insert(key, value).is_none())
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.records
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| DbError::NotFound("fake store key".into()))
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.records.lock().unwrap().remove(&key).is_some())
    }
    fn iter_stream(&self, _size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn scan_prefix_stream(&self, _prefix: Bytes, _size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream_reverse(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
}

#[async_trait]
impl Store for DiskImage {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let key = RecordKey::from_slice(&(self.pending.lock().unwrap().len() as u64).to_be_bytes());
        self.set(key, value).await?;
        Ok(key)
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        Ok(self.pending.lock().unwrap().insert(key, value).is_none())
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.pending
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| DbError::NotFound("fake store key".into()))
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.pending.lock().unwrap().remove(&key).is_some())
    }
    async fn flush(&self) -> DbResult<()> {
        if self.fail_flush.load(Ordering::Acquire) {
            return Err(DbError::Storage("injected flush failure".into()));
        }
        *self.durable.lock().unwrap() = self.pending.lock().unwrap().clone();
        Ok(())
    }
    fn iter_stream(&self, _size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn scan_prefix_stream(&self, _prefix: Bytes, _size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream_reverse(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
}

fn event(version: u64) -> Arc<ChangelogEvent> {
    Arc::new(ChangelogEvent {
        repo: "follower".into(),
        commit_version: version,
        tx_id: version,
        actor: Actor::System,
        timestamp_ns: 0,
        changes: vec![RecordChange {
            table: "items".into(),
            key: RecordId([version as u8; 16]).to_bytes(),
            op: ChangeOp::Put,
            value: None,
        }],
    })
}

#[tokio::test]
async fn verification_and_flush_make_events_and_certificate_survive_two_reopens() {
    let journal = DiskImage::default();
    let item = event(7);
    journal
        .set(
            RecordKey::from_slice(&7u64.to_be_bytes()),
            Bytes::from(rmp_serde::to_vec(item.as_ref()).unwrap()),
        )
        .await
        .unwrap();
    verify_events_and_flush(&journal, &[item], 7, 7)
        .await
        .unwrap();
    let metadata = Arc::new(DiskImage::default());
    let metadata_store: Arc<dyn Store> = metadata.clone();
    save_bookmark_and_clean_horizon(&metadata_store, 700, 7)
        .await
        .unwrap();
    let journal_reopen = journal.crash_image();
    let journal_key = RecordKey::from_slice(&7u64.to_be_bytes());
    let expected = Bytes::from(rmp_serde::to_vec(event(7).as_ref()).unwrap());
    assert_eq!(
        journal_reopen.get(journal_key.clone()).await.unwrap(),
        expected
    );
    for _ in 0..2 {
        let reopened_journal = journal_reopen.get(journal_key.clone()).await.unwrap();
        assert_eq!(reopened_journal, expected);
        let reopened_metadata = metadata.crash_image();
        assert_eq!(
            load_replication_bookmark(&reopened_metadata).await.unwrap(),
            700
        );
        assert_eq!(
            load_clean_horizon(&reopened_metadata).await.unwrap(),
            Some(7)
        );
    }
}

#[tokio::test]
async fn omitting_flush_is_a_negative_control_and_loses_unflushed_journal() {
    let journal = DiskImage::default();
    let event = event(3);
    journal
        .set(
            RecordKey::from_slice(&3u64.to_be_bytes()),
            Bytes::from(rmp_serde::to_vec(event.as_ref()).unwrap()),
        )
        .await
        .unwrap();
    verify_events_without_flush(&journal, &[event], 3, 3)
        .await
        .unwrap();
    assert!(matches!(
        journal
            .crash_image()
            .get(RecordKey::from_slice(&3u64.to_be_bytes()))
            .await,
        Err(DbError::NotFound(_))
    ));
}

#[tokio::test]
async fn missing_mismatched_and_failed_flush_refuse_certification() {
    let journal = DiskImage::default();
    assert!(verify_events_and_flush(&journal, &[event(1)], 1, 1)
        .await
        .is_err());
    journal
        .set(
            RecordKey::from_slice(&1u64.to_be_bytes()),
            Bytes::from_static(b"wrong"),
        )
        .await
        .unwrap();
    assert!(verify_events_and_flush(&journal, &[event(1)], 1, 1)
        .await
        .is_err());
    journal
        .set(
            RecordKey::from_slice(&1u64.to_be_bytes()),
            Bytes::from(rmp_serde::to_vec(event(1).as_ref()).unwrap()),
        )
        .await
        .unwrap();
    journal.fail_flush.store(true, Ordering::Release);
    assert!(verify_events_and_flush(&journal, &[event(1)], 1, 1)
        .await
        .is_err());
}

#[tokio::test]
async fn five_phase_crash_images_keep_old_or_certified_horizon() {
    for phase in 0..5 {
        let history = DiskImage::default();
        let journal = DiskImage::default();
        let metadata = Arc::new(DiskImage::default());
        let metadata_store: Arc<dyn Store> = metadata.clone();
        save_bookmark_and_clean_horizon(&metadata_store, 200, 2)
            .await
            .unwrap();
        let marker_key = RecordKey::from_slice(&99u64.to_be_bytes());
        history
            .set(marker_key.clone(), Bytes::from_static(b"history"))
            .await
            .unwrap();
        history.flush().await.unwrap();
        if phase >= 1 {
            let item = event(5);
            journal
                .set(
                    RecordKey::from_slice(&5u64.to_be_bytes()),
                    Bytes::from(rmp_serde::to_vec(item.as_ref()).unwrap()),
                )
                .await
                .unwrap();
        }
        if phase >= 2 {
            verify_events_and_flush(&journal, &[event(5)], 5, 5)
                .await
                .unwrap();
        }
        if phase >= 3 {
            save_bookmark_and_clean_horizon(&metadata_store, 500, 5)
                .await
                .unwrap();
        }

        let mut horizon = None;
        let mut bookmark = 0;
        let mut in_flight = None;
        for _ in 0..2 {
            let reopened_metadata = metadata.crash_image();
            horizon = load_clean_horizon(&reopened_metadata).await.unwrap();
            bookmark = load_replication_bookmark(&reopened_metadata).await.unwrap();
            in_flight = in_flight_range(horizon, 5);
            let reopened_journal = journal.crash_image();
            assert_eq!(
                reopened_journal
                    .get(RecordKey::from_slice(&5u64.to_be_bytes()))
                    .await
                    .is_ok(),
                phase >= 2,
                "phase {phase}"
            );
        }
        let reopened_history = history.crash_image();
        assert!(
            reopened_history.get(marker_key).await.is_ok(),
            "phase {phase}"
        );
        if phase >= 3 {
            assert_eq!((bookmark, horizon), (500, Some(5)));
            assert_eq!(in_flight, None);
        } else {
            assert_eq!((bookmark, horizon), (200, Some(2)));
            assert_eq!(in_flight, Some(3..=5));
        }
    }
}
