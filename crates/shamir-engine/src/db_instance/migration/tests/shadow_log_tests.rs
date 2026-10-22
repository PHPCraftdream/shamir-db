use async_trait::async_trait;
use bytes::Bytes;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;

use futures::{Stream, StreamExt};

use crate::migration::shadow_key::ShadowKey;
use crate::migration::shadow_log::{MigrationShadowLog, ShadowEntry, ShadowOp, READ_FROM_PAGE_CAP};

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn append_and_read_back() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m1".into(), store);

    let id1 = RecordId::new();
    let id2 = RecordId::new();
    let lsn1 = log
        .append(ShadowOp::Put {
            record_id: id1,
            value: b"hello".to_vec(),
        })
        .await
        .unwrap();
    let lsn2 = log
        .append(ShadowOp::Delete { record_id: id2 })
        .await
        .unwrap();

    assert_eq!(lsn1, 1);
    assert_eq!(lsn2, 2);
    assert_eq!(log.current_lsn(), 2);

    let entries = log.read_from(1).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].lsn, 1);
    assert_eq!(entries[1].lsn, 2);
}

#[tokio::test]
async fn read_from_filters_by_lsn() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m2".into(), store);

    for _ in 0..5 {
        log.append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    }

    let entries = log.read_from(3).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].lsn, 3);
    assert_eq!(entries[1].lsn, 4);
    assert_eq!(entries[2].lsn, 5);
}

#[tokio::test]
async fn append_batch_allocates_sequential_lsns() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m3".into(), store);

    let ops = vec![
        ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"a".to_vec(),
        },
        ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"b".to_vec(),
        },
        ShadowOp::Delete {
            record_id: RecordId::new(),
        },
    ];
    let lsns = log.append_batch(ops).await.unwrap();
    assert_eq!(lsns, vec![1, 2, 3]);
    assert_eq!(log.current_lsn(), 3);

    let entries = log.read_from(1).await.unwrap();
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn purge_removes_all_entries() {
    let store = mem_store();
    let log = MigrationShadowLog::new("m4".into(), store);

    for _ in 0..3 {
        log.append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    }

    let removed = log.purge().await.unwrap();
    assert_eq!(removed, 3);

    let entries = log.read_from(1).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn recover_restores_lsn_counter() {
    let store = mem_store();
    {
        let log = MigrationShadowLog::new("m5".into(), Arc::clone(&store));
        for _ in 0..5 {
            log.append(ShadowOp::Delete {
                record_id: RecordId::new(),
            })
            .await
            .unwrap();
        }
    }
    let log2 = MigrationShadowLog::recover("m5".into(), store)
        .await
        .unwrap();
    assert_eq!(log2.next_lsn(), 6);

    let lsn = log2
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    assert_eq!(lsn, 6);
}

#[tokio::test]
async fn separate_migration_ids_are_isolated() {
    let store = mem_store();
    let log_a = MigrationShadowLog::new("a".into(), Arc::clone(&store));
    let log_b = MigrationShadowLog::new("b".into(), store);

    log_a
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    log_b
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    log_b
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();

    assert_eq!(log_a.read_from(1).await.unwrap().len(), 1);
    assert_eq!(log_b.read_from(1).await.unwrap().len(), 2);
}

#[tokio::test]
async fn read_from_fails_closed_on_unrecognized_version_byte() {
    // Crash-recovery-critical path: an unrecognized version byte on a
    // shadow-log entry must fail closed with a coded error, never a
    // panic and never a silent misparse as the current `ShadowEntry`
    // shape.
    let store = mem_store();
    let log = MigrationShadowLog::new("m6".into(), Arc::clone(&store));

    let entry = ShadowEntry {
        lsn: 1,
        op: ShadowOp::Delete {
            record_id: RecordId::new(),
        },
    };
    let body = bincode::serialize(&entry).unwrap();
    let mut bad_bytes = vec![0xFFu8]; // unrecognized version byte
    bad_bytes.extend_from_slice(&body);
    let key = ShadowKey::new("m6", 1).to_record_key();
    store.set(key, bytes::Bytes::from(bad_bytes)).await.unwrap();

    let err = log.read_from(1).await.unwrap_err();
    assert!(
        matches!(err, DbError::Codec(_)),
        "expected a coded Codec error on unrecognized shadow_log version, got {err:?}"
    );
}

#[tokio::test]
async fn read_from_fails_closed_on_empty_entry_bytes() {
    // Defensive: an entry with zero bytes (missing even a version byte)
    // must also fail closed rather than panic on an out-of-bounds index.
    let store = mem_store();
    let log = MigrationShadowLog::new("m7".into(), Arc::clone(&store));

    let key = ShadowKey::new("m7", 1).to_record_key();
    store.set(key, bytes::Bytes::new()).await.unwrap();

    let err = log.read_from(1).await.unwrap_err();
    assert!(matches!(err, DbError::Codec(_)));
}

/// Defect 5 regression: production migration ids embed a user-controlled
/// table name (`mig_<table_name>_<ts>_<hex>`) that may itself contain
/// `_`. A migration id that is a byte-prefix of another id up to the old
/// delimiter (`"mig_users"` vs. `"mig_users_backup"`) must NOT see the
/// other migration's entries — this fails against the prior
/// `id || b'_' || lsn` key layout (`scan_prefix("mig_users")` is a byte-
/// prefix of every `"mig_users_backup"` key).
#[tokio::test]
async fn adversarial_prefix_ids_do_not_collide() {
    let store = mem_store();
    let log_a = MigrationShadowLog::new("mig_users".into(), Arc::clone(&store));
    let log_b = MigrationShadowLog::new("mig_users_backup".into(), store);

    log_a
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    log_b
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();
    log_b
        .append(ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .await
        .unwrap();

    assert_eq!(log_a.read_from(1).await.unwrap().len(), 1);
    assert_eq!(log_b.read_from(1).await.unwrap().len(), 2);
}

/// Wraps an `Arc<dyn Store>`, counting `scan_prefix_stream` invocations
/// and every `(key, value)` pair yielded by `iter_range_stream`'s
/// returned stream as the caller drains it.
struct RangeCountingStore {
    inner: Arc<dyn Store>,
    scan_prefix_calls: Arc<AtomicUsize>,
    range_items_yielded: Arc<AtomicUsize>,
}

#[async_trait]
impl Store for RangeCountingStore {
    async fn insert(&self, value: Bytes) -> shamir_storage::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> shamir_storage::error::DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> shamir_storage::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> shamir_storage::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    async fn set_many(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> shamir_storage::error::DbResult<Vec<bool>> {
        self.inner.set_many(items).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.scan_prefix_calls.fetch_add(1, SeqCst);
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        let inner_stream = self
            .inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size);
        let counter = Arc::clone(&self.range_items_yielded);
        Box::pin(inner_stream.inspect(move |batch| {
            if let Ok(items) = batch {
                counter.fetch_add(items.len(), SeqCst);
            }
        }))
    }
}

/// Defect 1 regression: `read_from(start_lsn)` must range-scan directly
/// from `(id, start_lsn)` — not re-scan the whole `__shadow_<id>_` prefix
/// from lsn 0 on every call — and must cap the number of entries
/// buffered in one call instead of returning an unbounded backlog.
#[tokio::test]
async fn read_from_range_scans_from_start_lsn_and_caps_page_size() {
    const TOTAL: u64 = (READ_FROM_PAGE_CAP as u64) * 3;
    const START_LSN: u64 = (READ_FROM_PAGE_CAP as u64) * 2 - (READ_FROM_PAGE_CAP as u64) / 2;

    let backing: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let scan_prefix_calls = Arc::new(AtomicUsize::new(0));
    let range_items_yielded = Arc::new(AtomicUsize::new(0));
    let counting: Arc<dyn Store> = Arc::new(RangeCountingStore {
        inner: backing,
        scan_prefix_calls: Arc::clone(&scan_prefix_calls),
        range_items_yielded: Arc::clone(&range_items_yielded),
    });
    let log = MigrationShadowLog::new("bigmig".into(), counting);

    let ops: Vec<ShadowOp> = (0..TOTAL)
        .map(|_| ShadowOp::Delete {
            record_id: RecordId::new(),
        })
        .collect();
    log.append_batch(ops).await.unwrap();

    let entries = log.read_from(START_LSN).await.unwrap();

    // Tail from START_LSN..=TOTAL is bigger than the page cap — result
    // must be capped, not the full tail.
    let full_tail_len = (TOTAL - START_LSN + 1) as usize;
    assert!(full_tail_len > READ_FROM_PAGE_CAP);
    assert_eq!(entries.len(), READ_FROM_PAGE_CAP);

    // Every returned entry is >= start_lsn and strictly ascending — the
    // range-scan is already sorted, no re-sort needed.
    assert!(entries.iter().all(|e| e.lsn >= START_LSN));
    assert!(entries.windows(2).all(|w| w[0].lsn < w[1].lsn));

    // No full-prefix scan happened (read_from no longer uses
    // scan_prefix_stream at all) and the range scan only pulled the
    // capped page's worth of items — not the whole log from lsn 0.
    assert_eq!(scan_prefix_calls.load(SeqCst), 0);
    assert_eq!(range_items_yielded.load(SeqCst), READ_FROM_PAGE_CAP);
}
