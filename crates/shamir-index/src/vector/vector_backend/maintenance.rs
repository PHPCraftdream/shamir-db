//! Background maintenance tasks for [`VectorBackend`]: the V2.3 (#402)
//! single-flight background snapshot (dump + generation flip + prune) and
//! the V4.2 (#408) background HNSW compaction.

use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::HnswAdapter;
use crate::vector::snapshot::{self, SnapshotError, SnapshotManifest};
use crate::vector::vector_backend::backend::{AdapterSlot, VectorBackend};
use arc_swap::{ArcSwap, ArcSwapOption};
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Resets the single-flight `snapshot_in_flight` flag on drop, so the flag is
/// cleared whether the background snapshot task returns `Ok`, returns `Err`, or
/// **panics** (unwinding still runs `Drop`). Without this, a panic inside the
/// dump would leak the flag as `true` permanently and disable all future
/// snapshots for the process (pillar: a guard must never stick).
pub(super) struct SnapshotFlightGuard(pub(super) Arc<AtomicBool>);

impl Drop for SnapshotFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// V2.3 (#402) — the body of the single-flight background snapshot task.
///
/// Reads the current manifest to learn the active gen + its chunk counts,
/// dumps a fresh generation (`gen+1`) from the live adapter, then atomically
/// flips the manifest to the new gen and prunes the old gen's chunks + every
/// delta chunk the new snapshot absorbed (index ≤ current HWM). The whole
/// flip is ONE `Store::transact` so a backend that overrides `transact`
/// exposes the new generation + the prune as a single all-or-nothing batch.
///
/// `adapter_arc` is the `Arc<dyn VectorAdapter>` captured at trigger time;
/// we downcast to `&HnswAdapter` inside (the Arc outlives the dump). The
/// `dump_snapshot_with_gen` call runs `file_dump` under `spawn_blocking`, so
/// no borrow is held across the flip's `transact`.
pub(super) async fn run_background_snapshot(
    info_store: &Arc<dyn Store>,
    keyspace: &str,
    adapter_arc: &Arc<dyn VectorAdapter>,
    next_delta_idx: &AtomicU64,
) -> Result<(), SnapshotError> {
    // The caller guaranteed the adapter is an HnswAdapter (checked before
    // spawn). Downcast once; hold the borrow for the dump below.
    let hnsw = adapter_arc
        .as_hnsw_adapter()
        .ok_or_else(|| SnapshotError::Backend("background snapshot on non-HnswAdapter".into()))?;

    // Read the current manifest to learn the active gen + chunk counts. A
    // missing manifest means no snapshot exists yet — we dump gen 1 and the
    // flip's "old gen" prune is a no-op (old_gen=0, 0 chunks).
    let (old_gen, old_graph_chunks, old_data_chunks): (u32, u32, u32) =
        match snapshot::read_manifest(info_store, keyspace).await {
            Ok(m) => (m.gen, m.graph_chunks, m.data_chunks),
            Err(SnapshotError::NotFound) => (0, 0, 0),
            Err(e) => return Err(e),
        };
    let new_gen = old_gen.wrapping_add(1);

    // The new snapshot absorbs every delta chunk written so far (indices
    // `0..next_delta_idx`). We capture the HWM BEFORE the dump so concurrent
    // Phase 5d appends (which bump `next_delta_idx`) land in chunks that
    // survive the flip's prune and are replayed on the next restart.
    // `delta_applied_upto` = number of absorbed chunks = `next_delta_idx`.
    let new_delta_applied_upto = next_delta_idx.load(Ordering::Acquire);

    // Dump the new generation. `dump_snapshot_with_gen` writes the chunks +
    // sidecar + a manifest for `new_gen` (with `delta_applied_upto = 0`). The
    // manifest it writes is at the single `manifest_key` slot — so it
    // OVERWRITES the old manifest. That is fine: `flip_generation` below
    // rewrites the manifest with the correct `delta_applied_upto` in the same
    // atomic batch as the prune, so a reader between the dump and the flip
    // sees `new_gen` with `delta_applied_upto = 0` (replays every delta —
    // correct, just slower on restart) and a reader after the flip sees the
    // final state. A crash between the dump and the flip leaves `new_gen`
    // published with `delta_applied_upto = 0`: the next restart loads
    // `new_gen` and replays every delta chunk (including the ones the dump
    // already absorbed) — idempotent under replay (upsert/delete are
    // last-write-wins), so no corruption, just redundant work.
    snapshot::dump_snapshot_with_gen(hnsw, info_store, keyspace, new_gen).await?;

    // Re-read the manifest dump_snapshot_with_gen just wrote to recover the
    // real chunk counts + basename (we don't duplicate the chunk-counting
    // logic here). Then patch in the correct `delta_applied_upto`.
    let written = snapshot::read_manifest(info_store, keyspace).await?;
    let final_manifest = SnapshotManifest {
        format_version: written.format_version,
        gen: written.gen,
        graph_chunks: written.graph_chunks,
        data_chunks: written.data_chunks,
        qgraph_chunks: written.qgraph_chunks,
        qdata_chunks: written.qdata_chunks,
        basename: written.basename,
        qbasename: written.qbasename,
        delta_applied_upto: new_delta_applied_upto,
    };

    // Atomic flip + prune. The old gen's chunks + sidecar are removed, every
    // delta chunk ≤ new_delta_applied_upto is removed, and the new manifest
    // is published — all in ONE transact.
    snapshot::flip_generation(
        info_store,
        keyspace,
        old_gen,
        old_graph_chunks,
        old_data_chunks,
        final_manifest,
        new_delta_applied_upto,
    )
    .await
}

// ---------------------------------------------------------------------------
// V4.2 (#408) — Background HNSW Compaction
// ---------------------------------------------------------------------------

/// Drop-guard that resets `compaction_in_flight` on Ok/Err/panic.
struct CompactionFlightGuard(Arc<AtomicBool>);

impl Drop for CompactionFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl VectorBackend {
    /// V4.2 (#408) — check whether compaction should trigger and spawn it.
    pub(crate) fn trigger_compaction_check(&self, info_store: &Arc<dyn Store>) {
        let adapter_arc = Arc::clone(&self.adapter.load_full().adapter);
        let hnsw = match adapter_arc.as_hnsw_adapter() {
            Some(h) => h,
            None => return,
        };

        let ratio_bits = self.compaction_ratio_threshold.load(Ordering::Acquire);
        let ratio_threshold = f64::from_bits(ratio_bits);
        let min_live = self.compaction_min_live.load(Ordering::Acquire) as usize;

        if hnsw.deleted_ratio() < ratio_threshold || hnsw.live_count() < min_live {
            return;
        }

        // Coordination: skip if snapshot in flight
        if self.snapshot_in_flight.load(Ordering::Acquire) {
            return;
        }

        // Single-flight
        if self
            .compaction_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let adapter_swap = Arc::clone(&self.adapter);
        let compaction_target = Arc::clone(&self.compaction_target);
        let compaction_in_flight = Arc::clone(&self.compaction_in_flight);
        let delta_count = Arc::clone(&self.delta_count);
        let next_delta_idx = Arc::clone(&self.next_delta_idx);
        let snapshot_in_flight = Arc::clone(&self.snapshot_in_flight);
        let info_store = Arc::clone(info_store);
        let keyspace = self.snapshot_keyspace();

        tokio::spawn(async move {
            let _flight_guard = CompactionFlightGuard(compaction_in_flight);
            let res = run_background_compaction(
                &adapter_swap,
                &compaction_target,
                &info_store,
                &keyspace,
                &delta_count,
                &next_delta_idx,
                &snapshot_in_flight,
            )
            .await;
            if let Err(e) = res {
                log::warn!(
                    "vector background compaction failed: {} — will retry on next threshold crossing",
                    e
                );
            }
        });
    }
}

/// V4.2 (#408) — the body of the background compaction task (Steps 1–7).
async fn run_background_compaction(
    adapter_swap: &Arc<ArcSwap<AdapterSlot>>,
    compaction_target: &Arc<ArcSwapOption<AdapterSlot>>,
    info_store: &Arc<dyn Store>,
    keyspace: &str,
    delta_count: &Arc<AtomicU64>,
    next_delta_idx: &Arc<AtomicU64>,
    snapshot_in_flight: &Arc<AtomicBool>,
) -> Result<(), String> {
    // Step 1: Create empty new adapter
    let old_slot = adapter_swap.load_full();
    let old_hnsw = old_slot
        .adapter
        .as_hnsw_adapter()
        .ok_or_else(|| "compaction on non-HnswAdapter".to_string())?;

    let config = old_hnsw.build_config();
    let dim = old_hnsw.dim_field();
    let metric = old_hnsw.metric_field();
    // #428 (VR-6) — quantization-aware compaction target. If the OLD adapter
    // was constructed with a quantization mode, the target inherits the SAME
    // mode (so the post-compaction snapshot is v2 with QuantMeta, and the
    // deferred-fit in `backfill_if_absent` re-trains the quantizer on the
    // surviving live set). `None` → legacy `new_compaction_target` (f32 path,
    // bit-for-bit back-compat with the pre-#428 compaction).
    let quant_mode = old_hnsw.quantization_mode();
    let new_adapter = HnswAdapter::new_compaction_target_quantized(dim, metric, config, quant_mode);
    let new_adapter_arc: Arc<dyn VectorAdapter> = Arc::new(new_adapter);

    // Step 2: Arm double-write
    compaction_target.store(Some(Arc::new(AdapterSlot {
        adapter: Arc::clone(&new_adapter_arc),
    })));

    // Step 3: Collect live-set S0 from OLD adapter
    let live_pairs = old_hnsw.collect_live_vectors();

    // Step 4a: Backfill S0 into new (INSERT-IF-ABSENT)
    let new_hnsw = new_adapter_arc
        .as_hnsw_adapter()
        .ok_or_else(|| "compaction target not HnswAdapter".to_string())?;
    new_hnsw
        .backfill_if_absent(&live_pairs)
        .await
        .map_err(|e| e.to_string())?;

    // Step 4b: Reconcile-deletes (close resurrect race)
    if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
        let mut rids_to_delete: Vec<RecordId> = Vec::new();
        del_rids.iter_sync(|rid, ()| {
            rids_to_delete.push(*rid);
            true
        });
        for rid in rids_to_delete {
            let _ = new_adapter_arc.delete(rid).await;
        }
    }

    // Step 5: Atomic swap (new becomes primary)
    adapter_swap.store(Arc::new(AdapterSlot {
        adapter: Arc::clone(&new_adapter_arc),
    }));

    // Step 6: Clear double-write (AFTER swap — order critical)
    compaction_target.store(None);

    // Step 7: Force snapshot from new adapter
    delta_count.store(0, Ordering::Release);
    // Only trigger if no snapshot already in flight
    if snapshot_in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let flight_guard = SnapshotFlightGuard(Arc::clone(snapshot_in_flight));
        let info_store = Arc::clone(info_store);
        let keyspace = keyspace.to_string();
        let adapter_for_snap = Arc::clone(&new_adapter_arc);
        let next_delta_idx = Arc::clone(next_delta_idx);
        let delta_count = Arc::clone(delta_count);
        tokio::spawn(async move {
            let _flight_guard = flight_guard;
            let res =
                run_background_snapshot(&info_store, &keyspace, &adapter_for_snap, &next_delta_idx)
                    .await;
            match res {
                Ok(()) => {
                    delta_count.store(0, Ordering::Release);
                }
                Err(e) => {
                    log::warn!(
                        "vector post-compaction snapshot failed for keyspace {}: {}",
                        keyspace,
                        e
                    );
                }
            }
        });
    }

    Ok(())
}
