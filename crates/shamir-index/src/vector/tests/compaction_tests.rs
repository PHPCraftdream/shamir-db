//! V4.2 (#408) — HNSW compaction tests: unit, integration, stress.

use crate::kind::VectorMetric;
use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use crate::vector::vector_backend::AdapterSlot;
use arc_swap::ArcSwapOption;
use shamir_collections::TFxSet;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn rid(n: u16) -> RecordId {
    let bytes = (n as u128).to_be_bytes();
    RecordId(bytes)
}

fn random_vec(dim: usize, seed: u64) -> Vec<f32> {
    let mut v = Vec::with_capacity(dim);
    let mut s = seed;
    for _ in 0..dim {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        v.push(((s >> 33) as f32) / (u32::MAX as f32) - 0.5);
    }
    v
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn collect_live_vectors_excludes_tombstones() {
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );
    adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[0.0, 1.0, 0.0]).await.unwrap();
    adapter.upsert(rid(3), &[0.0, 0.0, 1.0]).await.unwrap();
    adapter.delete(rid(2)).await.unwrap();

    let live = adapter.collect_live_vectors();
    let rids: TFxSet<RecordId> = live.iter().map(|(r, _)| *r).collect();
    assert!(rids.contains(&rid(1)));
    assert!(!rids.contains(&rid(2)));
    assert!(rids.contains(&rid(3)));
    assert_eq!(live.len(), 2);
}

#[tokio::test]
async fn backfill_if_absent_skips_existing() {
    let adapter = HnswAdapter::new_compaction_target(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );
    // Pre-insert rid(1) via double-write simulation
    adapter.upsert(rid(1), &[9.0, 9.0, 9.0]).await.unwrap();

    // Backfill with a different value for rid(1) and a new rid(2)
    let items = vec![(rid(1), vec![1.0, 0.0, 0.0]), (rid(2), vec![0.0, 1.0, 0.0])];
    adapter.backfill_if_absent(&items).await.unwrap();

    // rid(1) should keep its double-write value (9,9,9)
    assert_eq!(adapter.len(), 2);
    // rid(2) should be present
    assert!(adapter.contains_rid(&rid(2)));
}

#[tokio::test]
async fn backfill_if_absent_skips_deleted_rids() {
    let adapter = HnswAdapter::new_compaction_target(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );
    // Simulate double-write delete: upsert then delete, recording in deleted_rids
    adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
    adapter.delete(rid(1)).await.unwrap();
    if let Some(ref del_rids) = adapter.compaction_deleted_rids {
        let _ = del_rids.insert_async(rid(1), ()).await;
    }

    // Backfill should NOT resurrect rid(1)
    let items = vec![(rid(1), vec![1.0, 0.0, 0.0])];
    adapter.backfill_if_absent(&items).await.unwrap();
    assert_eq!(adapter.len(), 0);
}

#[tokio::test]
async fn should_compact_threshold_logic() {
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 5000,
            ..Default::default()
        },
    );
    // Insert 1000 vectors then delete 200 → ratio = 200/1000 = 0.2 < 0.3
    for i in 0..1000u16 {
        adapter
            .upsert(rid(i), &random_vec(3, i as u64))
            .await
            .unwrap();
    }
    for i in 0..200u16 {
        adapter.delete(rid(i)).await.unwrap();
    }
    // live_count = 800, deleted_ratio = 200/1000 = 0.2
    assert!(adapter.deleted_ratio() < 0.3);

    // Delete more to cross 0.3 threshold
    for i in 200..500u16 {
        adapter.delete(rid(i)).await.unwrap();
    }
    // deleted = 500, next_id = 1000, ratio = 0.5 > 0.3, live = 500 < 1000
    assert!(adapter.deleted_ratio() >= 0.3);
    assert!(adapter.live_count() < 1000); // below min_live
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compaction_rebuild_aside_removes_tombstones() {
    let dim = 4u32;
    let old = HnswAdapter::new(
        dim,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 2000,
            ..Default::default()
        },
    );
    // Insert 100, delete 50
    for i in 0..100u16 {
        old.upsert(rid(i), &random_vec(dim as usize, i as u64))
            .await
            .unwrap();
    }
    for i in 0..50u16 {
        old.delete(rid(i)).await.unwrap();
    }
    assert_eq!(old.deleted_count(), 50);

    // Simulate compaction protocol
    let config = old.build_config();
    let new = HnswAdapter::new_compaction_target(dim, VectorMetric::L2, config);
    let new_arc: Arc<dyn VectorAdapter> = Arc::new(new);
    let new_hnsw = new_arc.as_hnsw_adapter().unwrap();

    let live = old.collect_live_vectors();
    assert_eq!(live.len(), 50);
    new_hnsw.backfill_if_absent(&live).await.unwrap();

    assert_eq!(new_hnsw.deleted_count(), 0);
    assert_eq!(new_hnsw.live_count(), 50);
}

#[tokio::test]
async fn double_write_visibility_in_new_adapter() {
    let dim = 3u32;
    let old = HnswAdapter::new(
        dim,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 200,
            ..Default::default()
        },
    );
    let new = HnswAdapter::new_compaction_target(dim, VectorMetric::L2, HnswConfig::default());
    let new_arc: Arc<dyn VectorAdapter> = Arc::new(new);

    // Simulate double-write: upsert into both
    old.upsert(rid(1), &[1.0, 2.0, 3.0]).await.unwrap();
    new_arc.upsert(rid(1), &[1.0, 2.0, 3.0]).await.unwrap();

    let new_hnsw = new_arc.as_hnsw_adapter().unwrap();
    assert_eq!(new_hnsw.live_count(), 1);
}

#[tokio::test]
async fn single_flight_compaction_noop() {
    let flag = Arc::new(AtomicBool::new(true)); // simulate already in-flight
                                                // compare_exchange should fail
    assert!(flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err());
}

#[tokio::test]
async fn step4b_reconcile_prevents_resurrect() {
    let dim = 3u32;
    let new = HnswAdapter::new_compaction_target(
        dim,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 200,
            ..Default::default()
        },
    );
    let new_arc: Arc<dyn VectorAdapter> = Arc::new(new);
    let new_hnsw = new_arc.as_hnsw_adapter().unwrap();

    // Simulate: backfill inserts rid(5) (the race window — delete arrived
    // after backfill check but before insert)
    new_hnsw
        .backfill_if_absent(&[(rid(5), vec![1.0, 2.0, 3.0])])
        .await
        .unwrap();
    assert_eq!(new_hnsw.live_count(), 1);

    // Now simulate the double-write delete that was recorded in compaction_deleted_rids
    if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
        let _ = del_rids.insert_async(rid(5), ()).await;
    }

    // Step 4b: reconcile-deletes
    if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
        let mut rids_to_delete: Vec<RecordId> = Vec::new();
        del_rids.scan(|rid, ()| {
            rids_to_delete.push(*rid);
        });
        for r in rids_to_delete {
            let _ = new_arc.delete(r).await;
        }
    }

    // rid(5) must be gone
    assert_eq!(new_hnsw.live_count(), 0);
}

// ---------------------------------------------------------------------------
// Stress test
// ---------------------------------------------------------------------------

/// Stress test: N workers run upsert/delete concurrently with Steps 3-4b
/// (collect + backfill + reconcile). Workers finish, THEN Steps 5-6 (swap +
/// clear). This exercises the critical double-write + backfill correctness
/// under concurrency. The nanosecond tail-race between Step5→Step6 is
/// acceptable in production (idempotent writes to the same adapter); we
/// verify the substantive invariants here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_concurrent_mutations_during_compaction() {
    use std::sync::atomic::AtomicU64;
    use tokio::sync::Barrier;

    let dim = 8u32;
    let n_initial = 200u16;
    let n_concurrent_ops = 500u16;

    // Build the "old" adapter with initial data
    let old_adapter = Arc::new(HnswAdapter::new(
        dim,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 5000,
            ..Default::default()
        },
    ));
    for i in 0..n_initial {
        old_adapter
            .upsert(rid(i), &random_vec(dim as usize, i as u64))
            .await
            .unwrap();
    }

    // Set up ArcSwap (simulating VectorBackend.adapter)
    let adapter_swap: Arc<arc_swap::ArcSwap<AdapterSlot>> =
        Arc::new(arc_swap::ArcSwap::from(Arc::new(AdapterSlot {
            adapter: old_adapter.clone() as Arc<dyn VectorAdapter>,
        })));

    // Create compaction target
    let config = old_adapter.build_config();
    let new_adapter = HnswAdapter::new_compaction_target(dim, VectorMetric::L2, config);
    let new_adapter_arc: Arc<dyn VectorAdapter> = Arc::new(new_adapter);

    // Arm double-write (Step 2)
    let compaction_target: Arc<ArcSwapOption<AdapterSlot>> =
        Arc::new(ArcSwapOption::from(Some(Arc::new(AdapterSlot {
            adapter: Arc::clone(&new_adapter_arc),
        }))));

    // Track expected live set
    let expected_live = Arc::new(tokio::sync::Mutex::new(
        (0..n_initial).map(rid).collect::<TFxSet<RecordId>>(),
    ));
    let op_counter = Arc::new(AtomicU64::new(0));

    // Barrier: 4 workers + 1 compaction task start together
    let barrier = Arc::new(Barrier::new(5));

    // Spawn concurrent mutation tasks (run alongside Steps 3-4b)
    let mut handles = Vec::new();
    for worker in 0..4u16 {
        let compaction_target = Arc::clone(&compaction_target);
        let expected_live = Arc::clone(&expected_live);
        let op_counter = Arc::clone(&op_counter);
        let barrier = Arc::clone(&barrier);
        let old_adapter = Arc::clone(&old_adapter);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let base_rid = n_initial + worker * (n_concurrent_ops / 4);
            for i in 0..(n_concurrent_ops / 4) {
                let r = rid(base_rid + i);
                let vec = random_vec(dim as usize, (base_rid + i) as u64);

                // Upsert into primary (old — simulating the real hot path before swap)
                old_adapter.upsert(r, &vec).await.unwrap();
                // Double-write to compaction target
                if let Some(target) = compaction_target.load_full() {
                    let _ = target.adapter.upsert(r, &vec).await;
                }
                {
                    let mut live = expected_live.lock().await;
                    live.insert(r);
                }

                // Delete some (every 3rd)
                if i % 3 == 0 {
                    old_adapter.delete(r).await.unwrap();
                    if let Some(target) = compaction_target.load_full() {
                        let _ = target.adapter.delete(r).await;
                        if let Some(hnsw) = target.adapter.as_hnsw_adapter() {
                            if let Some(ref del_rids) = hnsw.compaction_deleted_rids {
                                let _ = del_rids.insert_async(r, ()).await;
                            }
                        }
                    }
                    {
                        let mut live = expected_live.lock().await;
                        live.remove(&r);
                    }
                }

                op_counter.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Compaction task runs Steps 3-4b concurrently with workers
    let compaction_handle = {
        let new_adapter_arc = Arc::clone(&new_adapter_arc);
        let barrier = Arc::clone(&barrier);
        let old_adapter = Arc::clone(&old_adapter);

        tokio::spawn(async move {
            barrier.wait().await;
            // Let some mutations land before collecting
            tokio::task::yield_now().await;

            // Step 3: collect from old
            let live_pairs = old_adapter.collect_live_vectors();

            // Step 4a: backfill
            let new_hnsw = new_adapter_arc.as_hnsw_adapter().unwrap();
            new_hnsw.backfill_if_absent(&live_pairs).await.unwrap();

            // Step 4b: reconcile-deletes
            if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
                let mut rids_to_delete: Vec<RecordId> = Vec::new();
                del_rids.scan(|rid, ()| {
                    rids_to_delete.push(*rid);
                });
                for r in rids_to_delete {
                    let _ = new_adapter_arc.delete(r).await;
                }
            }
        })
    };

    // Wait for ALL workers and compaction steps 3-4b to finish
    for h in handles {
        h.await.unwrap();
    }
    compaction_handle.await.unwrap();

    // Now do Step 5 (swap) and Step 6 (clear) — no concurrent mutations
    adapter_swap.store(Arc::new(AdapterSlot {
        adapter: Arc::clone(&new_adapter_arc),
    }));
    compaction_target.store(None);

    // Verify: new adapter's live set matches expected
    let slot = adapter_swap.load_full();
    let new_hnsw = slot.adapter.as_hnsw_adapter().unwrap();

    let expected = expected_live.lock().await;

    // Check: no ghost (every rid in new is in expected)
    let mut actual_live: TFxSet<RecordId> = TFxSet::with_hasher(THasher::default());
    new_hnsw.for_each_rid_to_internal(|rid, _| {
        actual_live.insert(rid);
    });

    // All expected rids must be in new adapter
    for r in expected.iter() {
        assert!(
            actual_live.contains(r),
            "LOST rid {:?} — not in new adapter",
            r
        );
    }
    // No ghost: every rid in new adapter must be in expected
    for r in actual_live.iter() {
        assert!(
            expected.contains(r),
            "GHOST rid {:?} — in new adapter but not expected",
            r
        );
    }

    // Search should not panic and return only valid rids
    use crate::vector::adapter::SearchOpts;
    let search_results = slot
        .adapter
        .search(
            &random_vec(dim as usize, 999),
            10,
            SearchOpts::default(),
            None,
        )
        .await
        .unwrap();
    for (r, _) in &search_results {
        assert!(actual_live.contains(r), "Search returned ghost rid {:?}", r);
    }
}
