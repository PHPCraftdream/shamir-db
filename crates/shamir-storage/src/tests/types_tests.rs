#![allow(deprecated)]

use crate::error::DbResult;
use crate::types::{RecordKey, Store};
use bytes::Bytes;
use futures::stream::Stream;
use futures::stream::StreamExt;
use std::pin::Pin;
use std::sync::Arc;

type RecordStream =
    Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>;

/// Collect all records from a stream into a single vector.
/// Used to convert iter_stream() results to a flat Vec.
///
/// **DEPRECATED & FOR TESTS ONLY**
///
/// WARNING: Only use in tests! Can consume all memory on large datasets.
#[deprecated(since = "0.1.0", note = "FOR TESTS ONLY.")]
pub async fn collect_stream(stream: RecordStream) -> DbResult<Vec<(RecordKey, Bytes)>> {
    let mut all_records = Vec::new();
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
    let new_id1 = shamir_types::types::record_id::RecordId::new();
    let new_id2 = shamir_types::types::record_id::RecordId::new();
    let new_k1 = Bytes::copy_from_slice(new_id1.as_bytes());
    let new_k2 = Bytes::copy_from_slice(new_id2.as_bytes());

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
    let missing_id = shamir_types::types::record_id::RecordId::new();
    let missing_k = Bytes::copy_from_slice(missing_id.as_bytes());
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
    let missing_id = shamir_types::types::record_id::RecordId::new();
    let missing_k = Bytes::copy_from_slice(missing_id.as_bytes());
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
        .map(|i| RecordKey::copy_from_slice(&[0xCC, i]))
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
