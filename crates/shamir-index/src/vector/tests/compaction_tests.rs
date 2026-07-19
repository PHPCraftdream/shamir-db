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
        let _ = del_rids.insert_sync(rid(1), ());
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
        let _ = del_rids.insert_sync(rid(5), ());
    }

    // Step 4b: reconcile-deletes
    if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
        let mut rids_to_delete: Vec<RecordId> = Vec::new();
        del_rids.iter_sync(|rid, ()| {
            rids_to_delete.push(*rid);
            true
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
                                let _ = del_rids.insert_sync(r, ());
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
                del_rids.iter_sync(|rid, ()| {
                    rids_to_delete.push(*rid);
                    true
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

/// #428 (VR-6, @sh adversarial-review coverage gap) — the SQ8 twin of
/// `stress_concurrent_mutations_during_compaction`: concurrent double-write
/// upsert/delete traffic hitting the compaction target WHILE
/// `backfill_if_absent`'s own deferred-fit (`try_fit_and_rebuild` — snapshot
/// → quantizer fit → u8 graph build → catch-up loop) is actually running,
/// not merely quiescent.
///
/// `@sh`'s review of the VR-6 fix reasoned the mechanism is safe by
/// construction (the same `fit_in_flight` single-flight CAS and
/// `claim_and_publish_u8` dedup machinery VR-1 already hardened against
/// concurrent plain `upsert` traffic crossing the fit boundary), but flagged
/// that no test actually exercised a QUANTIZED target under this exact
/// combination — this test closes that gap empirically rather than leaving
/// the safety argument purely structural.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_concurrent_mutations_during_quantized_compaction() {
    use std::sync::atomic::AtomicU64;
    use tokio::sync::Barrier;

    let dim = 8u32;
    // Comfortably above FIT_THRESHOLD (256) so backfill_if_absent's
    // deferred-fit check fires DURING Step 4a, concurrently with the
    // worker traffic below — the exact window @sh flagged as untested.
    let n_initial = 300u16;
    let n_concurrent_ops = 200u16;

    let old_adapter = Arc::new(HnswAdapter::new_with_quantization(
        dim,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 5000,
            ..Default::default()
        },
        Some(VectorQuantization::Sq8),
    ));
    for i in 0..n_initial {
        old_adapter
            .upsert(rid(i), &random_vec(dim as usize, i as u64))
            .await
            .unwrap();
    }
    assert!(
        old_adapter.is_quantized(),
        "test setup: old adapter must have fitted (n_initial > FIT_THRESHOLD)"
    );

    let adapter_swap: Arc<arc_swap::ArcSwap<AdapterSlot>> =
        Arc::new(arc_swap::ArcSwap::from(Arc::new(AdapterSlot {
            adapter: old_adapter.clone() as Arc<dyn VectorAdapter>,
        })));

    // Compaction target inherits quantization (#428 fix) — starts un-fitted,
    // fits DURING backfill once the live set crosses FIT_THRESHOLD.
    let config = old_adapter.build_config();
    let new_adapter = HnswAdapter::new_compaction_target_quantized(
        dim,
        VectorMetric::L2,
        config,
        old_adapter.quantization_mode(),
    );
    let new_adapter_arc: Arc<dyn VectorAdapter> = Arc::new(new_adapter);

    let compaction_target: Arc<ArcSwapOption<AdapterSlot>> =
        Arc::new(ArcSwapOption::from(Some(Arc::new(AdapterSlot {
            adapter: Arc::clone(&new_adapter_arc),
        }))));

    let expected_live = Arc::new(tokio::sync::Mutex::new(
        (0..n_initial).map(rid).collect::<TFxSet<RecordId>>(),
    ));
    let op_counter = Arc::new(AtomicU64::new(0));
    // Barrier: 4 upsert/delete workers + 1 dedicated delete-only hammer
    // (H3 regression extension) + 1 compaction task = 6.
    let barrier = Arc::new(Barrier::new(6));

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

                old_adapter.upsert(r, &vec).await.unwrap();
                if let Some(target) = compaction_target.load_full() {
                    let _ = target.adapter.upsert(r, &vec).await;
                }
                {
                    let mut live = expected_live.lock().await;
                    live.insert(r);
                }

                if i % 3 == 0 {
                    old_adapter.delete(r).await.unwrap();
                    if let Some(target) = compaction_target.load_full() {
                        let _ = target.adapter.delete(r).await;
                        if let Some(hnsw) = target.adapter.as_hnsw_adapter() {
                            if let Some(ref del_rids) = hnsw.compaction_deleted_rids {
                                let _ = del_rids.insert_sync(r, ());
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

    // H3 regression extension: dedicated DELETE-ONLY hammer targeting
    // `compaction_deleted_rids`'s hazard specifically. This map is genuinely
    // shared/hammered during double-write (unlike the other four maps'
    // spread-key nature), so a dedicated delete task increases the
    // contention on `insert_sync` racing `contains_sync` (backfill guard)
    // and `iter_sync` (Step4b reconcile) — the exact mixed-sync-accessor
    // pattern the fix addresses. Deletes target existing initial rids,
    // simulating user deletes during the compaction window.
    {
        let compaction_target = Arc::clone(&compaction_target);
        let expected_live = Arc::clone(&expected_live);
        let barrier = Arc::clone(&barrier);
        let old_adapter = Arc::clone(&old_adapter);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            for i in 0..(n_concurrent_ops / 2) {
                let r = rid(i % n_initial);
                old_adapter.delete(r).await.unwrap();
                if let Some(target) = compaction_target.load_full() {
                    if let Some(hnsw) = target.adapter.as_hnsw_adapter() {
                        if let Some(ref del_rids) = hnsw.compaction_deleted_rids {
                            let _ = del_rids.insert_sync(r, ());
                        }
                    }
                    let _ = target.adapter.delete(r).await;
                }
                {
                    let mut live = expected_live.lock().await;
                    live.remove(&r);
                }
            }
        }));
    }

    let compaction_handle = {
        let new_adapter_arc = Arc::clone(&new_adapter_arc);
        let barrier = Arc::clone(&barrier);
        let old_adapter = Arc::clone(&old_adapter);

        tokio::spawn(async move {
            barrier.wait().await;
            tokio::task::yield_now().await;

            let live_pairs = old_adapter.collect_live_vectors();

            // Step 4a: backfill — the 300-vector live set crosses
            // FIT_THRESHOLD (256) INSIDE this call, at the SAME time worker
            // tasks are double-writing into `new_adapter_arc` above. This is
            // the exact interleaving @sh flagged as untested.
            let new_hnsw = new_adapter_arc.as_hnsw_adapter().unwrap();
            new_hnsw.backfill_if_absent(&live_pairs).await.unwrap();

            if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
                let mut rids_to_delete: Vec<RecordId> = Vec::new();
                del_rids.iter_sync(|rid, ()| {
                    rids_to_delete.push(*rid);
                    true
                });
                for r in rids_to_delete {
                    let _ = new_adapter_arc.delete(r).await;
                }
            }
        })
    };

    for h in handles {
        h.await.unwrap();
    }
    compaction_handle.await.unwrap();

    adapter_swap.store(Arc::new(AdapterSlot {
        adapter: Arc::clone(&new_adapter_arc),
    }));
    compaction_target.store(None);

    let slot = adapter_swap.load_full();
    let new_hnsw = slot.adapter.as_hnsw_adapter().unwrap();

    // The target MUST have fitted — quantization survived the compaction
    // under concurrent load (the #428 fix's whole point).
    assert!(
        new_hnsw.is_quantized(),
        "compaction target failed to fit under concurrent load — \
         quantization lost (the #428 regression)"
    );

    let expected = expected_live.lock().await;
    let mut actual_live: TFxSet<RecordId> = TFxSet::with_hasher(THasher::default());
    new_hnsw.for_each_rid_to_internal(|rid, _| {
        actual_live.insert(rid);
    });

    // No loss: every expected rid must be present (Б-1-class invariant —
    // this is exactly what a double-claim/premature-convergence race under
    // concurrent compaction+fit would violate).
    for r in expected.iter() {
        assert!(
            actual_live.contains(r),
            "LOST rid {:?} — not in quantized compaction target",
            r
        );
    }
    for r in actual_live.iter() {
        assert!(
            expected.contains(r),
            "GHOST rid {:?} — in quantized compaction target but not expected",
            r
        );
    }

    // Search must not panic and must not return ghosts — proves the u8
    // graph is connected (Б-1 invariant) even though it was built while
    // concurrent traffic was double-writing into the same adapter.
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
        assert!(
            actual_live.contains(r),
            "Search returned ghost rid {:?} from quantized compaction target",
            r
        );
    }
}

// ===========================================================================
// #428 (VR-6) — quantization-aware compaction
//
// Before #428, `run_background_compaction` ALWAYS built the target via
// `new_compaction_target` (f32), so a compaction on a fitted SQ8 index
// silently dropped quantization: memory returned to 4×, and the post-
// compaction snapshot was v1 (no QuantMeta). These tests cover the fix:
// the target inherits the old adapter's quantization mode, backfill
// re-fits the quantizer on the surviving live set (Variant A), the f32
// graph is dropped post-fit, and the snapshot after compaction is v2.
// Back-compat (non-quant compaction stays f32) is also asserted.
// ===========================================================================

use crate::kind::VectorQuantization;
use crate::vector::adapter::SearchOpts;

/// Deterministic Gaussian-clustered vector generator (mirrors
/// `quantized_graph_tests::Lcg`) — needed so the SQ8 quantizer has a
/// non-degenerate distribution to fit on.
struct Lcg {
    state: u64,
}
impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
    #[inline]
    fn next_f32(&mut self) -> f32 {
        let high = (self.next_u64() >> 32) as u32;
        (high as f32) / (1u64 << 32) as f32
    }
    #[inline]
    fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
    fn next_gaussian(&mut self) -> f32 {
        loop {
            let u1 = self.next_f32() * 2.0 - 1.0;
            let u2 = self.next_f32() * 2.0 - 1.0;
            let s = u1 * u1 + u2 * u2;
            if s > 0.0 && s < 1.0 {
                let mul = ((-2.0 * s.ln()) / s).sqrt();
                return u1 * mul;
            }
        }
    }
}

fn clustered(n: usize, dim: usize, k: usize, sigma: f32, seed: u64) -> Vec<Vec<f32>> {
    assert!(k > 0);
    let mut rng = Lcg::new(seed);
    let centroids: Vec<Vec<f32>> = (0..k)
        .map(|_| (0..dim).map(|_| rng.next_range(-1.0, 1.0)).collect())
        .collect();
    (0..n)
        .map(|i| {
            let c = &centroids[i % k];
            (0..dim)
                .map(|j| c[j] + sigma * rng.next_gaussian())
                .collect()
        })
        .collect()
}

/// Build a FITTED SQ8 adapter with `n` live vectors, then tombstone the first
/// `n_delete` of them — modelling a dirty index that a compaction would
/// trigger on. Returns the adapter and the full dataset (for recall queries).
async fn build_fitted_sq8_dirty(
    n: usize,
    n_delete: usize,
    dim: u32,
    seed: u64,
) -> (HnswAdapter, Vec<Vec<f32>>) {
    let adapter = HnswAdapter::new_with_quantization(
        dim,
        VectorMetric::Cosine,
        HnswConfig {
            max_elements: 10_000,
            m: 16,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 128,
        },
        Some(VectorQuantization::Sq8),
    );
    let data = clustered(n, dim as usize, 10, 0.2, seed);
    let items: Vec<(RecordId, Vec<f32>)> = data
        .iter()
        .enumerate()
        .map(|(i, v)| (rid(i as u16), v.clone()))
        .collect();
    adapter.upsert_batch(&items).await.unwrap();
    assert!(adapter.is_quantized(), "test setup: adapter did not fit");
    for i in 0..n_delete {
        adapter.delete(rid(i as u16)).await.unwrap();
    }
    (adapter, data)
}

/// Run the adapter-level compaction rebuild (Steps 1–4a of
/// `run_background_compaction`) on `old`, producing the new fitted target.
/// Mirrors what the production compaction path does, minus the Store/snapshot
/// plumbing.
async fn compaction_rebuild(old: &HnswAdapter, dim: u32) -> HnswAdapter {
    let config = old.build_config();
    let metric = old.metric_field();
    let q = old.quantization_mode();
    let new = HnswAdapter::new_compaction_target_quantized(dim, metric, config, q);
    let live_pairs = old.collect_live_vectors();
    new.backfill_if_absent(&live_pairs).await.unwrap();
    new
}

/// #428 — compaction of a fitted SQ8 index produces a QUANTIZED target.
/// The target re-fits the quantizer on the surviving live set (Variant A),
/// so `is_quantized()` is true and the f32 graph is dropped (the deterministic
/// memory-regression invariant from #418).
#[tokio::test]
async fn vr6_sq8_compaction_produces_quantized_target() {
    let dim = 32u32;
    // 400 live, 100 tombstoned → 300 survivors, well above FIT_THRESHOLD (256).
    let (old, _data) = build_fitted_sq8_dirty(400, 100, dim, 0xA11CE).await;
    assert_eq!(old.deleted_count(), 100);
    assert_eq!(old.live_count(), 300);

    let new = compaction_rebuild(&old, dim).await;

    // Variant A: the target re-fit on the 300 survivors → quantized.
    assert!(
        new.is_quantized(),
        "#428: SQ8 compaction target did not re-fit — quantization lost"
    );
    assert!(
        new.quantizer().is_some(),
        "#428: SQ8 compaction target has no quantizer"
    );
    assert!(
        new.hnsw_u8_handle().is_some(),
        "#428: SQ8 compaction target has no u8 graph"
    );
    // #418 memory invariant — the f32 graph must be dropped post-fit.
    assert!(
        !new.f32_graph_present(),
        "#428: SQ8 compaction target retained the f32 graph (4× memory regression)"
    );
    // Live count preserved across compaction.
    assert_eq!(new.live_count(), 300);
    assert_eq!(new.deleted_count(), 0);
}

/// #428 — search is correct after compaction + re-fit. The recall of the
/// post-compaction target (queried on the surviving vectors) must be high
/// against the pre-compaction adapter's results on the same survivors.
#[tokio::test]
async fn vr6_sq8_compaction_recall_smoke() {
    let dim = 32u32;
    let (old, data) = build_fitted_sq8_dirty(400, 100, dim, 0xB0B).await;
    let new = compaction_rebuild(&old, dim).await;
    assert!(new.is_quantized());

    let opts = SearchOpts {
        ef_search: Some(256),
        oversample: None,
    };
    // Query the first 10 surviving vectors (rids 100..109 survived: deletes
    // were 0..99). Compare top-10 overlap between old and new.
    let mut hits = 0usize;
    let mut poss = 0usize;
    for (i, q) in data.iter().enumerate().take(110).skip(100) {
        let before = old.search(q, 10, opts, None).await.unwrap();
        let after = new.search(q, 10, opts, None).await.unwrap();
        let before_set: TFxSet<RecordId> = before.into_iter().map(|(r, _)| r).collect();
        for (r, _) in &after {
            if before_set.contains(r) {
                hits += 1;
            }
            poss += 1;
        }
        let _ = i; // index unused beyond positioning; silence warnings on refactor
    }
    let recall = hits as f32 / poss.max(1) as f32;
    // SQ8 + compaction re-fit; the bar matches the existing
    // `recall_sq8_vs_f32_within_two_percent` tolerance band (allowing for
    // hnsw_rs unseedable-RNG variance).
    assert!(
        recall >= 0.80,
        "#428: post-compaction recall@10 = {recall:.4} below 0.80"
    );
}

/// #428 — snapshot after compaction of a quantized index is v2 (QuantMeta
/// present). Mirrors `compaction_snapshot_restart_preserves_quant_state` but
/// drives the compaction rebuild through the #428 path (quantized target +
/// deferred-fit in backfill) before dumping.
#[tokio::test]
async fn vr6_sq8_compaction_snapshot_is_v2() {
    use crate::meta_envelope::MetaEnvelope;
    use crate::vector::snapshot::{
        self, dump_snapshot, load_snapshot, SnapshotSidecar, SNAPSHOT_FORMAT_VERSION,
    };
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;

    const KS: &str = "vsnapq.vr6";
    let dim = 32u32;
    let (old, _data) = build_fitted_sq8_dirty(400, 100, dim, 0xC0FFEE).await;
    let new = compaction_rebuild(&old, dim).await;
    assert!(new.is_quantized(), "precondition: target re-fitted");

    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    dump_snapshot(&new, &store, KS).await.unwrap();

    // Sidecar must stamp format_version = 2 and carry QuantMeta.
    let sidecar_k = snapshot::sidecar_key_for_test(KS, 0);
    let sidecar_bytes = store.get(sidecar_k).await.unwrap();
    let sidecar: SnapshotSidecar = MetaEnvelope::open(&sidecar_bytes).unwrap();
    assert_eq!(
        sidecar.format_version, SNAPSHOT_FORMAT_VERSION,
        "#428: post-compaction snapshot format_version != SNAPSHOT_FORMAT_VERSION"
    );
    assert!(
        sidecar.quantization.is_some(),
        "#428: post-compaction v2 sidecar has no quantization (QuantMeta lost)"
    );

    // Round-trip: load produces a fitted quantized adapter.
    let loaded = load_snapshot(&store, KS).await.unwrap();
    assert!(
        loaded.is_quantized(),
        "#428: post-compaction v2 load did not restore a fitted adapter"
    );
}

/// #428 — back-compat: compaction of a NON-quantized index leaves the target
/// on the f32 path (not quantized, f32 graph resident). The existing
/// non-quant compaction tests must not regress.
#[tokio::test]
async fn vr6_non_quant_compaction_stays_f32() {
    let dim = 8u32;
    let old = HnswAdapter::new(
        dim,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 5000,
            ..Default::default()
        },
    );
    // Insert 400, delete 100 → 300 live.
    for i in 0..400u16 {
        old.upsert(rid(i), &random_vec(dim as usize, i as u64))
            .await
            .unwrap();
    }
    for i in 0..100u16 {
        old.delete(rid(i)).await.unwrap();
    }
    assert!(old.quantization_mode().is_none());

    let new = compaction_rebuild(&old, dim).await;

    // Non-quant → target must NOT be quantized and the f32 graph must stay.
    assert!(
        !new.is_quantized(),
        "#428 back-compat: non-quant compaction target became quantized"
    );
    assert!(
        new.f32_graph_present(),
        "#428 back-compat: non-quant compaction target dropped the f32 graph"
    );
    assert_eq!(new.live_count(), 300);
    assert_eq!(new.deleted_count(), 0);
}

// ===========================================================================
// Post-VR-8 follow-up — delete-vs-backfill same-rid race (found by the final
// 10x `@vector @engine --full` stress gate, then confirmed by @sh
// adversarial review of the `backfill_if_absent` per-item dispatch fix).
//
// `backfill_if_absent`'s self-migration re-check (mirrors `upsert`, Б-1/#423)
// MUST guard the claim on `!self.deleted.contains(&internal)`, exactly like
// `upsert` does — without the guard, a concurrent `delete(r)` on the SAME rid
// backfill just claimed (reachable via the double-write compaction protocol)
// tombstones `internal` (bumping `migrated_pre_flip` once via
// `bump_migrated_on_tombstone`) BEFORE backfill's own re-check runs; the
// re-check's claim then wins a SECOND time (vectors_u8 was never populated
// by the tombstone path) and bumps `migrated_pre_flip` again — a double
// count that can trip the catch-up loop's `migrated >= target` convergence
// early, and inserts a live graph node for an already-tombstoned internal.
//
// This is deliberately NOT covered by `stress_concurrent_mutations_during_
// quantized_compaction` above — that test's delete workers only ever target
// rids THEY themselves inserted via double-write (rids >= n_initial), never
// the rids backfill_if_absent is concurrently processing (rids < n_initial).
// This test closes exactly that gap: delete workers race the SAME rid range
// backfill is backfilling, for the whole duration of the backfill call.
// ===========================================================================
#[tokio::test]
async fn backfill_delete_same_rid_race_no_double_count() {
    // Hang guard (matches the codebase convention for this test class, e.g.
    // `concurrent_upsert_with_tombstone_across_fit_does_not_hang`): a
    // convergence bug must surface as an actionable test FAILURE within a
    // bounded window, never as an indefinite hang burning CI/dev wall-clock.
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let dim = 8u32;
        let target = Arc::new(HnswAdapter::new_compaction_target_quantized(
            dim,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 5000,
                ..Default::default()
            },
            Some(VectorQuantization::Sq8),
        ));

        // The exact rid range backfill_if_absent will process — deliberately
        // BELOW FIT_THRESHOLD (256) alone, so the flip must come from the
        // concurrent upsert workers below (crossing threshold via THEIR OWN
        // deferred-fit trigger), landing mid-backfill-loop — exactly the
        // window the (now-fixed) missing guard left open. Kept small: this
        // test only needs ONE delete to land inside the narrow race window,
        // not scale for its own sake — a large item count just multiplies
        // real spawn_blocking round-trips without improving reproduction odds.
        let n_backfill = 40u16;
        let backfill_items: Vec<(RecordId, Vec<f32>)> = (0..n_backfill)
            .map(|i| (rid(i), random_vec(dim as usize, i as u64)))
            .collect();

        let backfill_target = Arc::clone(&target);
        let backfill_handle = tokio::spawn(async move {
            backfill_target
                .backfill_if_absent(&backfill_items)
                .await
                .unwrap();
        });

        // Push the target past FIT_THRESHOLD via a DIFFERENT rid range so
        // the adapter's own deferred-fit fires concurrently with (not
        // after) backfill's loop above.
        let mut upsert_handles = Vec::new();
        for w in 0..4u16 {
            let target = Arc::clone(&target);
            upsert_handles.push(tokio::spawn(async move {
                for i in 0..70u16 {
                    let r = rid(1000 + w * 100 + i);
                    let _ = target
                        .upsert(r, &random_vec(dim as usize, (2000 + w * 100 + i) as u64))
                        .await;
                }
            }));
        }

        // Race deletes against the SAME rid range backfill is backfilling —
        // the missing guard's failure window is narrow (between backfill's
        // Vacant claim and its self-migration re-check), so a handful of
        // concurrent passes over the small range is enough to have a
        // realistic shot at landing inside it without inflating runtime.
        let mut delete_handles = Vec::new();
        for _ in 0..4u16 {
            let target = Arc::clone(&target);
            delete_handles.push(tokio::spawn(async move {
                for i in 0..n_backfill {
                    let _ = target.delete(rid(i)).await;
                }
            }));
        }

        backfill_handle.await.unwrap();
        for h in upsert_handles {
            h.await.unwrap();
        }
        for h in delete_handles {
            h.await.unwrap();
        }

        target
    })
    .await;

    let target = result.expect(
        "backfill_delete_same_rid_race_no_double_count timed out (30s) — \
         convergence never reached migrated_pre_flip >= next_id_at_flip, \
         a regression in the catch-up loop or the claim-guard fix",
    );

    // The invariant a double-count would violate: migrated_pre_flip must
    // never exceed next_id_at_flip once fitted. (Pre-fix, this could also
    // manifest as `backfill_if_absent` itself returning an Internal error
    // — the `.unwrap()` above already guards that regression class.)
    if target.is_quantized() {
        let (migrated, target_count) = target.test_convergence_counters();
        assert!(
            migrated <= target_count,
            "migrated_pre_flip ({migrated}) exceeded next_id_at_flip \
             ({target_count}) — double-counted a claim (missing \
             !self.deleted.contains(&internal) guard in backfill_if_absent's \
             self-migration re-check)"
        );
    }
}
