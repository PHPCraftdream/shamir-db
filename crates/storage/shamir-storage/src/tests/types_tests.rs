#![allow(deprecated)]

use crate::error::{DbError, DbResult};
use crate::types::{RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::StreamExt;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct RangeProbe {
    polls: Arc<AtomicUsize>,
    batches: Vec<Vec<u8>>,
    fail_after_batches: bool,
}

#[async_trait]
impl Store for RangeProbe {
    async fn insert(&self, _: Bytes) -> DbResult<RecordKey> {
        unreachable!()
    }

    async fn set(&self, _: RecordKey, _: Bytes) -> DbResult<bool> {
        unreachable!()
    }

    async fn get(&self, _: RecordKey) -> DbResult<Bytes> {
        unreachable!()
    }

    async fn remove(&self, _: RecordKey) -> DbResult<bool> {
        unreachable!()
    }

    fn iter_stream(&self, _: usize) -> RecordStream {
        let polls = Arc::clone(&self.polls);
        let mut batches = self.batches.clone().into_iter();
        let fail_after_batches = self.fail_after_batches;
        Box::pin(futures::stream::poll_fn(move |_| {
            polls.fetch_add(1, Ordering::SeqCst);
            let item = batches
                .next()
                .map(|batch| {
                    Ok(batch
                        .into_iter()
                        .map(|key| {
                            (
                                RecordKey::from_slice(&[key]),
                                Bytes::copy_from_slice(&[key]),
                            )
                        })
                        .collect())
                })
                .or_else(|| {
                    fail_after_batches.then(|| Err(DbError::Storage("after bound".into())))
                });
            std::task::Poll::Ready(item)
        }))
    }

    fn scan_prefix_stream(&self, _: Bytes, _: usize) -> RecordStream {
        unreachable!()
    }
}

#[tokio::test]
async fn default_range_stops_polling_after_upper_bound() {
    for (batches, expected_polls, expected) in [
        (vec![vec![1, 2, 3, 4]], 1, vec![2, 3]),
        (vec![vec![1, 2, 3], vec![4]], 2, vec![2, 3]),
        (vec![vec![4]], 1, vec![]),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        let store = RangeProbe {
            polls: Arc::clone(&polls),
            batches,
            fail_after_batches: true,
        };
        let mut stream = store.iter_range_stream(
            Some(Bytes::from_static(&[2])),
            Some(Bytes::from_static(&[3])),
            4,
        );
        let mut keys = Vec::new();
        while let Some(batch) = stream.next().await {
            keys.extend(
                batch
                    .expect("no error after upper bound")
                    .into_iter()
                    .map(|(key, _)| key.as_ref()[0]),
            );
        }
        assert_eq!(keys, expected);
        assert_eq!(polls.load(Ordering::SeqCst), expected_polls);
        assert!(stream.next().await.is_none());
        assert_eq!(polls.load(Ordering::SeqCst), expected_polls);
    }
}

#[tokio::test]
async fn default_range_preserves_unbounded_and_error_behavior() {
    let polls = Arc::new(AtomicUsize::new(0));
    let store = RangeProbe {
        polls: Arc::clone(&polls),
        batches: vec![vec![1, 2], vec![3, 4]],
        fail_after_batches: false,
    };
    let records = collect_stream(store.iter_range_stream(Some(Bytes::from_static(&[2])), None, 2))
        .await
        .unwrap();
    assert_eq!(
        records
            .into_iter()
            .map(|(key, _)| key.as_ref()[0])
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(polls.load(Ordering::SeqCst), 3);

    let store = RangeProbe {
        polls: Arc::new(AtomicUsize::new(0)),
        batches: vec![vec![1]],
        fail_after_batches: true,
    };
    let mut stream = store.iter_range_stream(None, Some(Bytes::from_static(&[3])), 2);
    assert!(stream.next().await.unwrap().is_ok());
    assert!(matches!(
        stream.next().await,
        Some(Err(DbError::Storage(_)))
    ));

    let mut empty = store.iter_range_stream(None, None, 0);
    assert!(empty.next().await.is_none());
}

/// Collect all records from a stream into a single vector.
/// Used to convert iter_stream() results to a flat Vec.
///
/// **DEPRECATED & FOR TESTS ONLY**
///
/// WARNING: Only use in tests! Can consume all memory on large datasets.
#[deprecated(since = "0.1.0", note = "FOR TESTS ONLY.")]
pub async fn collect_stream(stream: RecordStream) -> DbResult<Vec<(RecordKey, Bytes)>> {
    let mut all_records = Vec::with_capacity(128);
    let mut stream = stream;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        all_records.extend(batch);
    }
    Ok(all_records)
}

/// Backend-agnostic test coverage for the `insert_many`, `set_many`,
/// `remove_many`, and `flush` trait methods. Each backend test
/// invokes this helper to verify both the default-loop impl and any
/// native overrides behave identically.
///
/// WARNING: tests only.
pub async fn run_batch_store_tests(store: Arc<dyn Store>) {
    // ---- insert_many --------------------------------------------------
    let values: Vec<Bytes> = (0..5u8)
        .map(|i| Bytes::copy_from_slice(&[i, i + 1, i + 2]))
        .collect();
    let keys = store
        .insert_many(values.clone())
        .await
        .expect("insert_many");
    assert_eq!(keys.len(), 5);

    // Every returned key is readable and round-trips to its value.
    for (k, v) in keys.iter().zip(values.iter()) {
        let got = store.get(k.clone()).await.expect("get after insert_many");
        assert_eq!(got.as_ref(), v.as_ref());
    }
    // Keys are unique.
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 5, "insert_many returned duplicate keys");

    // Empty input must return empty output (no transaction, no fsync).
    let empty = store
        .insert_many(Vec::new())
        .await
        .expect("insert_many empty");
    assert!(empty.is_empty());

    // ---- set_many -----------------------------------------------------
    // Mix: update existing 2, create new 2.
    let new_id1 = RecordId::new();
    let new_id2 = RecordId::new();
    let new_k1 = RecordKey::from_slice(new_id1.as_bytes());
    let new_k2 = RecordKey::from_slice(new_id2.as_bytes());

    let items = vec![
        (keys[0].clone(), Bytes::from_static(b"updated-0")),
        (keys[1].clone(), Bytes::from_static(b"updated-1")),
        (new_k1.clone(), Bytes::from_static(b"fresh-1")),
        (new_k2.clone(), Bytes::from_static(b"fresh-2")),
    ];
    let flags = store.set_many(items).await.expect("set_many");
    assert_eq!(flags, vec![false, false, true, true]);

    // Values landed.
    assert_eq!(
        store.get(keys[0].clone()).await.unwrap().as_ref(),
        b"updated-0"
    );
    assert_eq!(
        store.get(new_k2.clone()).await.unwrap().as_ref(),
        b"fresh-2"
    );

    // Empty input.
    let empty_set = store.set_many(Vec::new()).await.expect("set_many empty");
    assert!(empty_set.is_empty());

    // ---- remove_many --------------------------------------------------
    let missing_id = RecordId::new();
    let missing_k = RecordKey::from_slice(missing_id.as_bytes());
    let to_remove = vec![keys[2].clone(), keys[3].clone(), missing_k];
    let remove_flags = store.remove_many(to_remove).await.expect("remove_many");
    assert_eq!(remove_flags, vec![true, true, false]);
    assert!(store.get(keys[2].clone()).await.is_err());
    assert!(store.get(keys[3].clone()).await.is_err());

    // Empty input.
    let empty_rm = store
        .remove_many(Vec::new())
        .await
        .expect("remove_many empty");
    assert!(empty_rm.is_empty());

    // ---- flush --------------------------------------------------------
    // Must succeed on any backend. After flush, prior writes remain
    // visible (consistency, not just durability).
    store.flush().await.expect("flush");
    assert_eq!(
        store.get(keys[0].clone()).await.unwrap().as_ref(),
        b"updated-0",
        "data lost across flush"
    );

    // ---- get_many -----------------------------------------------------
    // Mix of hits (the just-set keys) and a missing key. Result must
    // preserve input order: Some(bytes) per hit, None per miss.
    let missing_id = RecordId::new();
    let missing_k = RecordKey::from_slice(missing_id.as_bytes());
    let probe_keys = vec![
        keys[0].clone(),   // hit — was set to "updated-0"
        missing_k.clone(), // miss
        new_k1.clone(),    // hit — set to "fresh-1"
        new_k2.clone(),    // hit — set to "fresh-2"
    ];
    let got = store.get_many(probe_keys.clone()).await.expect("get_many");
    assert_eq!(got.len(), 4, "get_many length mismatch");
    assert_eq!(got[0].as_deref(), Some(&b"updated-0"[..]));
    assert_eq!(got[1], None, "missing key must be None");
    assert_eq!(got[2].as_deref(), Some(&b"fresh-1"[..]));
    assert_eq!(got[3].as_deref(), Some(&b"fresh-2"[..]));

    // Empty input → empty output, no I/O.
    let empty = store.get_many(Vec::new()).await.expect("get_many empty");
    assert!(empty.is_empty());

    // ---- iter_range_stream_reverse ------------------------------------
    // Insert a small set of records with predictable keys, then walk
    // them via the reverse stream and assert order is high → low.
    // Must work both for the default impl (in-memory / cached /
    // fjall / persy / canopy / nebari) and the native overrides on
    // sled / redb.
    let mut rev_keys: Vec<RecordKey> = (0u8..8)
        .map(|i| RecordKey::from_slice(&[0xCC, i]))
        .collect();
    for (i, k) in rev_keys.iter().enumerate() {
        store
            .set(k.clone(), Bytes::copy_from_slice(&[i as u8]))
            .await
            .expect("seed reverse");
    }
    // Build range bounds covering exactly the seeded prefix.
    let lower = Bytes::copy_from_slice(&[0xCC, 0x00]);
    let upper = Bytes::copy_from_slice(&[0xCC, 0xFF]);
    let stream = store.iter_range_stream_reverse(Some(lower), Some(upper), 3);
    futures::pin_mut!(stream);
    let mut collected: Vec<RecordKey> = Vec::new();
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.expect("reverse batch") {
            collected.push(k);
        }
    }
    assert_eq!(
        collected.len(),
        8,
        "iter_range_stream_reverse returned {} entries, expected 8",
        collected.len()
    );
    rev_keys.sort();
    rev_keys.reverse();
    assert_eq!(
        collected, rev_keys,
        "iter_range_stream_reverse did not yield keys in high→low order"
    );
}
