//! Background maintenance tasks for [`VectorBackend`]: the V2.3 (#402)
//! single-flight background snapshot (dump + generation flip + prune) and
//! the V4.2 (#408) background HNSW compaction.

use crate::backend::IndexError;
use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::HnswAdapter;
use crate::vector::snapshot::{self, SnapshotError, SnapshotManifest};
use crate::vector::vector_backend::backend::VectorBackend;
use crate::vector::vector_backend::cutover::{disarm_publish, publish_slot, seal_and_drain};

#[cfg(test)]
use crate::vector::vector_backend::cutover::CompactionHooks;
use crate::vector::vector_backend::slot::AdapterSlot;
use crate::vector::vector_backend::target::CompactionTarget;
use arc_swap::ArcSwap;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

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
///
/// P0 delta-prune (brief 164): `applied_prefix` is the contiguous APPLIED
/// prefix owned by the backend (`mark_delta_applied`), NOT the allocation
/// counter `next_delta_idx`. It is sampled BEFORE the dump and that exact
/// sampled value is used for BOTH `manifest.delta_applied_upto` and the
/// prune bound — a chunk whose graph promote has not acknowledged it yet
/// (pending or failed) stays replayable and is never pruned.
pub(super) async fn run_background_snapshot(
    info_store: &Arc<dyn Store>,
    keyspace: &str,
    adapter_arc: &Arc<dyn VectorAdapter>,
    applied_prefix: &AtomicU64,
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

    // The new snapshot absorbs every delta chunk the graph has APPLIED so
    // far (indices `0..applied_prefix`). The boundary is sampled BEFORE the
    // dump and the exact sampled value drives both the manifest and the
    // prune, so a chunk appended concurrently (or promoted into a graph the
    // dump cannot see yet) always survives the flip and replays on restart.
    // Sampling `next_delta_idx` here instead would prune chunks whose
    // mutations have not reached the dumped graph — data loss on restart
    // (P0, brief 164).
    let new_delta_applied_upto = applied_prefix.load(Ordering::Acquire);

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

impl VectorBackend {
    /// RV-INDEX-2F3CBCA3 on the rebuild fallback: the rebuilt graph comes
    /// from the data store, but `next_delta_idx` must still start above the
    /// persisted absorption boundary and any surviving chunk, or the next
    /// `append_delta` lands below the boundary and is skipped by replay.
    /// A scan error is propagated (never defaulted to 0).
    /// P0 delta-prune: the applied prefix seeds alongside it — the fallback
    /// replayed/rebuilt the whole graph, so every surviving chunk is applied.
    pub(super) async fn seed_delta_idx_for_fallback(
        &self,
        index_name: &str,
        info_store: &Arc<dyn Store>,
        keyspace: &str,
        manifest: Option<&SnapshotManifest>,
    ) -> Result<(), IndexError> {
        let stored_hwm = snapshot::highest_delta_index(info_store, keyspace)
            .await
            .map_err(|e| {
                IndexError::Backend(format!(
                    "delta HWM scan failed for index {} (keyspace {}): {} —                      refusing to seed next_delta_idx below the surviving chunks",
                    index_name, keyspace, e
                ))
            })?;
        // Fresh index (no manifest, no chunks): keep the constructor seed.
        if manifest.is_none() && stored_hwm == 0 {
            return Ok(());
        }
        let boundary = manifest.map_or(0, |m| m.delta_applied_upto);
        let seed = boundary.max(stored_hwm).saturating_add(1);
        self.next_delta_idx.store(seed, Ordering::Release);
        self.applied_prefix.store(seed, Ordering::Release);
        Ok(())
    }

    /// P0 delta-prune (brief 164): acknowledge delta chunk `idx` as applied
    /// to the live graph and advance the contiguous applied prefix across
    /// the acknowledged run. Out-of-order acknowledgements park in
    /// `applied_set` until the prefix reaches them; a permanently failed
    /// (never acknowledged) promotion leaves its chunk un-pruned and
    /// replayable forever — the boundary simply stays below it.
    /// Lock-free: `scc::TreeIndex` + a `fetch_update`-style CAS loop on the
    /// prefix (no mutex on this hot path).
    pub(super) fn ack_delta_applied(&self, idx: u64) {
        if idx < self.applied_prefix.load(Ordering::Acquire) {
            return; // already inside the contiguous prefix
        }
        let _ = self.applied_set.insert_sync(idx, ());
        loop {
            let p = self.applied_prefix.load(Ordering::Acquire);
            if !self.applied_set.contains(&p) {
                return;
            }
            if self
                .applied_prefix
                .compare_exchange(p, p + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.applied_set.remove_sync(&p);
            }
        }
    }

    /// Contiguous applied prefix (observability): the count of leading delta
    /// chunks acknowledged as applied to the live graph — exactly the value
    /// the next snapshot samples as its prune boundary.
    pub fn applied_prefix(&self) -> u64 {
        self.applied_prefix.load(Ordering::Acquire)
    }

    /// The prune boundary handed to EVERY background snapshot — the
    /// production spawn (`trigger_snapshot_check`) AND the inline seam
    /// ([`Self::snapshot_now`]). Single source of truth so the inline
    /// snapshot exercises exactly the same boundary wiring as the spawned
    /// task AND the compaction trigger; a revert here to
    /// `Arc::clone(&self.next_delta_idx)` (the old allocation counter) breaks
    /// every prune-boundary regression test, including the post-compaction one.
    pub(super) fn prune_boundary(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.applied_prefix)
    }

    /// Inline variant of `trigger_snapshot_check` (test/administration
    /// seam): run the background-snapshot body against the CURRENT adapter,
    /// with the SAME coordination as the spawned path — skipped (no-op
    /// `Ok`) while a compaction is in flight, single-flight via
    /// `snapshot_in_flight` (a second concurrent call is a no-op `Ok`), and
    /// the flight guard is cleared on every exit. The prune bound is
    /// [`Self::prune_boundary`] — the contiguous applied prefix.
    #[doc(hidden)]
    pub async fn snapshot_now(&self, info_store: &Arc<dyn Store>) -> Result<(), SnapshotError> {
        if self.compaction_in_flight.load(Ordering::Acquire) {
            return Ok(());
        }
        if self
            .snapshot_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        let _flight_guard = SnapshotFlightGuard(Arc::clone(&self.snapshot_in_flight));
        let adapter_arc = Arc::clone(&self.adapter.load_full().adapter);
        let keyspace = self.snapshot_keyspace();
        let applied_prefix = self.prune_boundary();
        run_background_snapshot(info_store, &keyspace, &adapter_arc, &applied_prefix).await
    }

    /// Test-only handles for driving `run_background_compaction` directly:
    /// `(delta_count, applied_prefix, snapshot_in_flight, fail_next_backfill)`.
    #[cfg(test)]
    pub(crate) fn compaction_task_inputs_for_test(
        &self,
    ) -> (
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
    ) {
        (
            Arc::clone(&self.delta_count),
            self.prune_boundary(),
            Arc::clone(&self.snapshot_in_flight),
            Arc::clone(&self.fail_next_backfill),
        )
    }
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
        let published = Arc::clone(&self.published);
        let compaction_in_flight = Arc::clone(&self.compaction_in_flight);
        let delta_count = Arc::clone(&self.delta_count);
        let applied_prefix = self.prune_boundary();
        let snapshot_in_flight = Arc::clone(&self.snapshot_in_flight);
        #[cfg(test)]
        let fail_next_backfill = Arc::clone(&self.fail_next_backfill);
        #[cfg(test)]
        let compaction_hooks = Arc::clone(&self.compaction_hooks);
        let info_store = Arc::clone(info_store);
        let keyspace = self.snapshot_keyspace();

        tokio::spawn(async move {
            let _flight_guard = CompactionFlightGuard(compaction_in_flight);
            let res = run_background_compaction(
                &adapter_swap,
                &published,
                &info_store,
                &keyspace,
                &delta_count,
                &applied_prefix,
                &snapshot_in_flight,
                #[cfg(test)]
                &fail_next_backfill,
                #[cfg(test)]
                Some(compaction_hooks),
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

/// V4.2 (#408) — the body of the background compaction task, reworked for
/// RV-INDEX-9F62C8DF (binding @oh design):
///
/// 1. build the target adapter privately;
/// 2. **arm**: seal+drain g0's gate, then CAS-publish the armed slot g1 =
///    `{g0.adapter, Some(target), fresh gate}` — one linearizable
///    publication, no separately-sampled mirror pointer. F1: the slot is
///    wrapped in an Arc ONCE and `publish_slot` publishes that exact Arc,
///    which the coordinator retains — every later seal/drain/CAS targets
///    the slot that is really current;
/// 3. capture the live set S0 **after** that publish (a write drained
///    during the seal is visible to backfill);
/// 4. backfill: any error disarms (seal, drain, publish a slot without
///    target — retrying the CAS against the freshly loaded slot on a lost
///    race, never ignoring it) and returns Err — an armed-but-unbackfilled
///    target is never published;
///
/// Step 4b: reconcile deletes only for RIDs still absent from the old
///    primary — F4: the RID is CLAIMED on the target FIRST (a claim failure
///    fails the target, same ordering-ambiguity rule as writers), then the
///    presence check runs, then the mirror delete, then the claim is
///    released — closing the reinsertion window;
/// 5. **cutover**: seal+drain g1; if the target failed, disarm-publish
///    `{old, None}` and return Err (compaction retried later); else publish
///    `{new, None}`. Nothing fallible between seal and publish;
/// 7. snapshot of the published primary.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_background_compaction(
    adapter_swap: &Arc<ArcSwap<AdapterSlot>>,
    published: &Arc<Notify>,
    info_store: &Arc<dyn Store>,
    keyspace: &str,
    delta_count: &Arc<AtomicU64>,
    applied_prefix: &Arc<AtomicU64>,
    snapshot_in_flight: &Arc<AtomicBool>,
    #[cfg(test)] fail_next_backfill: &AtomicBool,
    #[cfg(test)] hooks: Option<Arc<CompactionHooks>>,
) -> Result<(), String> {
    // Step 1: g0 + private target adapter.
    let g0 = adapter_swap.load_full();
    let old_hnsw = g0
        .adapter
        .as_hnsw_adapter()
        .ok_or_else(|| "compaction on non-HnswAdapter".to_string())?;

    let config = old_hnsw.build_config();
    let dim = old_hnsw.dim_field();
    let metric = old_hnsw.metric_field();
    // #428 (VR-6) — quantization-aware compaction target (see the V4.2
    // history): the target inherits the OLD adapter's quantization mode.
    let quant_mode = old_hnsw.quantization_mode();
    let new_adapter = HnswAdapter::new_compaction_target_quantized(dim, metric, config, quant_mode);
    let new_adapter_arc: Arc<dyn VectorAdapter> = Arc::new(new_adapter);
    let target = CompactionTarget::new(Arc::clone(&new_adapter_arc));

    // Step 2: arm — seal g0, drain in-flight leases, publish g1 atomically.
    // F1: g1 is wrapped ONCE; publish_slot publishes that exact Arc.
    #[cfg(test)]
    if let Some(h) = hooks.as_deref() {
        h.fire_before_seal();
    }
    seal_and_drain(&g0).await;
    let g1 = Arc::new(AdapterSlot::armed(
        Arc::clone(&g0.adapter),
        Arc::clone(&target),
    ));
    if publish_slot(adapter_swap, &g0, Arc::clone(&g1), published).is_none() {
        return Err("compaction arm publication lost the slot CAS".to_string());
    }
    // F5: instance-scoped hook — hand the coordinator's OWN target to the
    // test, then park until the test releases (T2/T8 determinism).
    #[cfg(test)]
    if let Some(h) = hooks.as_deref() {
        h.fire_after_arm(Arc::clone(&target)).await;
    }

    // Step 3: live set S0 AFTER the arm publish.
    let live_pairs = old_hnsw.collect_live_vectors();

    // Step 4a: backfill S0 into new (INSERT-IF-ABSENT). Any error disarms.
    let new_hnsw = new_adapter_arc
        .as_hnsw_adapter()
        .ok_or_else(|| "compaction target not HnswAdapter".to_string())?;
    let backfill = async {
        #[cfg(test)]
        if fail_next_backfill.swap(false, Ordering::AcqRel) {
            return Err("injected backfill failure (fail_next_backfill)".to_string());
        }
        new_hnsw
            .backfill_if_absent(&live_pairs)
            .await
            .map_err(|e| e.to_string())
    };
    if let Err(e) = backfill.await {
        // Disarm (F1): seal, drain, publish a slot WITHOUT the target,
        // retrying the CAS against the freshly loaded slot on a lost race
        // (`disarm_publish` never ignores the CAS result). The old primary
        // stays authoritative; compaction retries on the next crossing.
        disarm_publish(adapter_swap, published).await;
        return Err(format!("compaction backfill failed: {e}"));
    }

    // Step 4b: reconcile-deletes, but ONLY for RIDs still absent from the
    // old primary — the historical tombstone set records "deleted
    // sometime", not "currently absent", so a blanket delete would undo a
    // successful re-insertion. A mirror error fails the target.
    if let Some(ref del_rids) = new_hnsw.compaction_deleted_rids {
        let mut rids_to_delete: Vec<RecordId> = Vec::new();
        del_rids.iter_sync(|rid, ()| {
            rids_to_delete.push(*rid);
            true
        });
        for rid in rids_to_delete {
            // F4: claim the RID on the target FIRST. A failed claim means a
            // concurrent writer holds this RID inside the generation —
            // ordering ambiguity, fail the target (same rule as writers).
            // This closes the reinsertion window: a concurrent reinsertion
            // either happens before the presence check (skipped below) or
            // sees this claim and fails the target (never published).
            if !target.claim(rid) {
                target.fail(format!(
                    "reconcile delete raced a live writer for rid {rid:?}"
                ));
                break;
            }
            if old_hnsw.contains_rid(&rid) {
                target.release(rid); // re-inserted after the delete: tombstone lost
                continue;
            }
            target.mirror_delete(rid).await;
            target.release(rid);
        }
    }

    // Step 5: cutover — seal g1, drain its leases, publish atomically.
    seal_and_drain(&g1).await;
    // F5: instance-scoped after-seal PAIR — the coordinator parks between
    // the drain and the publish until the test releases (T4).
    #[cfg(test)]
    if let Some(h) = hooks.as_deref() {
        h.fire_after_seal().await;
    }
    if target.failed() {
        // A failed target is NEVER published: keep the current primary,
        // drop the mirror, let compaction retry later (F1: CAS loss is
        // retried against the freshly loaded slot, not ignored).
        let reason = target.fail_reason().unwrap_or("unknown").to_string();
        disarm_publish(adapter_swap, published).await;
        return Err(format!(
            "compaction target failed; old primary retained: {reason}"
        ));
    }
    if publish_slot(
        adapter_swap,
        &g1,
        Arc::new(AdapterSlot::live(Arc::clone(&new_adapter_arc))),
        published,
    )
    .is_none()
    {
        return Err("compaction cutover publication lost the slot CAS".to_string());
    }

    // Step 7: Force snapshot from new adapter. F7: the delta counter is NOT
    // reset here — only the snapshot task resets it, AFTER snapshot success.
    // Only trigger if no snapshot already in flight
    if snapshot_in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let flight_guard = SnapshotFlightGuard(Arc::clone(snapshot_in_flight));
        let info_store = Arc::clone(info_store);
        let keyspace = keyspace.to_string();
        let adapter_for_snap = Arc::clone(&new_adapter_arc);
        let applied_prefix = Arc::clone(applied_prefix);
        let delta_count = Arc::clone(delta_count);
        tokio::spawn(async move {
            let _flight_guard = flight_guard;
            let res =
                run_background_snapshot(&info_store, &keyspace, &adapter_for_snap, &applied_prefix)
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
