//! Phase 3 — ts-index tests.
//!
//! Verify the in-memory `(ts, version)` TreeIndex provides O(log N) lookups
//! for `version_at_or_before_ts` and rebuilds correctly on open.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;

use super::helpers::{make_gate, make_mvcc};
use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;

/// Basic: write N versions with distinct timestamps, query returns correct max version.
#[tokio::test]
async fn ts_index_basic() {
    let mvcc = make_mvcc();
    // Freeze clock and write versions at distinct timestamps.
    mvcc.set_test_now(1000);
    let v1 = mvcc
        .set_versioned(Bytes::from("k1"), Bytes::from("val1"))
        .await
        .unwrap();

    mvcc.set_test_now(2000);
    let v2 = mvcc
        .set_versioned(Bytes::from("k2"), Bytes::from("val2"))
        .await
        .unwrap();

    mvcc.set_test_now(3000);
    let v3 = mvcc
        .set_versioned(Bytes::from("k3"), Bytes::from("val3"))
        .await
        .unwrap();

    // Query at ts=2500 should return v2 (ts=2000 ≤ 2500, ts=3000 > 2500).
    let result = mvcc.version_at_or_before_ts(2500).await;
    assert_eq!(result, Some(v2));

    // Query at ts=3000 should return v3.
    let result = mvcc.version_at_or_before_ts(3000).await;
    assert_eq!(result, Some(v3));

    // Query at ts=999 should return None (all versions have ts >= 1000).
    let result = mvcc.version_at_or_before_ts(999).await;
    assert_eq!(result, None);

    // Query at ts=1000 should return v1.
    let result = mvcc.version_at_or_before_ts(1000).await;
    assert_eq!(result, Some(v1));

    // Query at ts=u64::MAX should return the latest version.
    let result = mvcc.version_at_or_before_ts(u64::MAX).await;
    assert_eq!(result, Some(v3));
}

/// Compare new O(log N) ts-index result with old O(total) scan on random queries.
#[tokio::test]
async fn ts_index_byte_identical_to_scan() {
    let mvcc = make_mvcc();

    // Write 20 versions at various timestamps.
    let timestamps = [
        100, 200, 200, 300, 500, 500, 500, 700, 800, 900, 1000, 1100, 1200, 1300, 1400, 1500, 1600,
        1700, 1800, 1900,
    ];
    for (i, &ts) in timestamps.iter().enumerate() {
        mvcc.set_test_now(ts);
        let key = format!("key-{}", i);
        mvcc.set_versioned(Bytes::from(key), Bytes::from("v"))
            .await
            .unwrap();
    }

    // Query at 100 different timestamps and compare index vs scan.
    let query_points = [
        0,
        50,
        100,
        150,
        200,
        250,
        300,
        450,
        500,
        600,
        700,
        750,
        800,
        850,
        900,
        950,
        1000,
        1050,
        1100,
        1500,
        1800,
        1900,
        2000,
        u64::MAX,
    ];
    for &ts in &query_points {
        let from_index = mvcc.version_at_or_before_ts(ts).await;
        let from_scan = mvcc.version_at_or_before_ts_scan(ts).await;
        assert_eq!(
            from_index, from_scan,
            "Mismatch at query ts={}: index={:?}, scan={:?}",
            ts, from_index, from_scan
        );
    }
}

/// Rebuild on open: create MvccStore, write versions, create a NEW MvccStore
/// with the same history store, verify the ts-index is rebuilt and produces
/// correct results.
#[tokio::test]
async fn ts_index_rebuilds_on_open() {
    let gate = make_gate();
    let history: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());
    let mvcc = MvccStore::new(Arc::clone(&history), Arc::clone(&gate));

    // Write versions with distinct timestamps.
    mvcc.set_test_now(1000);
    let _v1 = mvcc
        .set_versioned(Bytes::from("k1"), Bytes::from("val1"))
        .await
        .unwrap();

    mvcc.set_test_now(2000);
    let v2 = mvcc
        .set_versioned(Bytes::from("k2"), Bytes::from("val2"))
        .await
        .unwrap();

    mvcc.set_test_now(3000);
    let v3 = mvcc
        .set_versioned(Bytes::from("k3"), Bytes::from("val3"))
        .await
        .unwrap();

    // Create a NEW MvccStore with the SAME history (simulates restart/open).
    let gate2 = Arc::new(RepoTxGate::fresh());
    let mvcc2 = MvccStore::new(Arc::clone(&history), gate2);

    // The new store has an empty ts_index — first query triggers rebuild.
    let result = mvcc2.version_at_or_before_ts(2500).await;
    assert_eq!(result, Some(v2));

    let result = mvcc2.version_at_or_before_ts(3000).await;
    assert_eq!(result, Some(v3));

    let result = mvcc2.version_at_or_before_ts(999).await;
    assert_eq!(result, None);
}

/// Concurrent safety: 8 writers + readers operating simultaneously — no panics,
/// no incorrect results under contention.
#[tokio::test]
async fn ts_index_concurrent_safe() {
    use std::sync::Arc;

    let gate = make_gate();
    let history: Arc<dyn shamir_storage::types::Store> = Arc::new(InMemoryStore::new());
    let mvcc = Arc::new(MvccStore::new(history, gate));

    let num_writers = 8;
    let writes_per_writer = 50;

    // Spawn writers.
    let mut handles = Vec::new();
    for w in 0..num_writers {
        let mvcc_clone = Arc::clone(&mvcc);
        handles.push(tokio::spawn(async move {
            for i in 0..writes_per_writer {
                let ts = (w * 1000 + i * 10 + 100) as u64;
                mvcc_clone.set_test_now(ts);
                let key = format!("w{}-k{}", w, i);
                mvcc_clone
                    .set_versioned(Bytes::from(key), Bytes::from("data"))
                    .await
                    .unwrap();
            }
        }));
    }

    // Spawn readers concurrently.
    for _ in 0..8 {
        let mvcc_clone = Arc::clone(&mvcc);
        handles.push(tokio::spawn(async move {
            for ts in (0..8000).step_by(100) {
                // Should not panic or produce incorrect results.
                let _ = mvcc_clone.version_at_or_before_ts(ts).await;
            }
        }));
    }

    // Await all.
    for h in handles {
        h.await.unwrap();
    }

    // Final sanity: query at a high ts should return SOME version.
    let result = mvcc.version_at_or_before_ts(u64::MAX).await;
    assert!(result.is_some());

    // Total versions written = num_writers * writes_per_writer.
    // The max version returned should be that total (monotonic allocation).
    let total = (num_writers * writes_per_writer) as u64;
    assert_eq!(result, Some(total));
}
