//! `IndexBackend` wrapper around any `VectorAdapter`.
//!
//! Extracts the vector field from records, delegates to the adapter
//! for similarity search. Returns `IndexResult::Ranked`.

use super::adapter::{VectorAdapter, VectorError};
use super::snapshot::{self, SnapshotError, SnapshotManifest};
use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::write_ops::IndexWriteOp;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures::StreamExt;
use shamir_storage::types::Store;
use shamir_tunables::instance_defaults::VECTOR_SNAPSHOT_DELTA_THRESHOLD;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::{RecordRef, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Keyspace prefix every vector-snapshot record lives under in the info
/// store. The full keyspace is `__vec_snap__<index_id>` where `<index_id>`
/// is the `IndexDescriptor::id` (a stable, persisted, auto-incremented u32).
/// Using the numeric id — not the human name — keeps the keyspace stable
/// across renames and immune to path/encoding concerns.
///
/// `snapshot::dump_snapshot` / `load_snapshot` prefix every chunk, sidecar,
/// and manifest key with `<keyspace>.`, so the actual on-disk keys look like
/// `__vec_snap__7.g0.graph.000000`, `__vec_snap__7.g0.sidecar`,
/// `__vec_snap__7.manifest`.
const SNAPSHOT_KEYSPACE_PREFIX: &str = "__vec_snap__";

/// Sized handle wrapping an `Arc<dyn VectorAdapter>`, so it can live
/// inside an `ArcSwap` (whose `RefCnt` impl requires `T: Sized` —
/// `arc-swap 1.9.1` does not support unsized `Arc<dyn Trait>` directly).
///
/// This is a plain (non-transparent) struct: the outer `Arc<AdapterSlot>`
/// is a thin pointer to a sized allocation that itself owns the
/// `Arc<dyn VectorAdapter>`. `ArcSwap<Arc<AdapterSlot>>` uses the
/// crate-provided `RefCnt for Arc<T>` (which works for sized `T`); the
/// adapter is recovered via `.adapter` on load.
///
/// Why a PERMANENT `ArcSwap` (not a one-shot construct-with-loaded-adapter):
/// V2.2 only hot-swaps once on open (before the backend sees traffic), but the
/// slot stays swappable on purpose — #402 (background re-snapshot / generation
/// flip) and #408 (compaction rebuild-aside) swap a freshly-built graph into a
/// LIVE backend. Readers take a snapshot via `load_full()` (an `Arc`, never a
/// `Guard` held across `.await`), so a concurrent swap is wait-free RCU.
pub(crate) struct AdapterSlot {
    pub adapter: Arc<dyn VectorAdapter>,
}

pub struct VectorBackend {
    descriptor: IndexDescriptor,
    field_path: Vec<u64>,
    /// Hot-swappable adapter (RCU snapshot pattern). The open path's
    /// `restore_on_open` swaps a freshly-loaded snapshot adapter in for the
    /// empty placeholder constructed by `build_index2_backend_with_resolver`;
    /// every read-side method grabs a cheap `load()` snapshot, so a
    /// concurrent query never sees a half-swapped adapter. Mirrors
    /// `BruteForceAdapter`'s use of `ArcSwap`.
    ///
    /// Wrapped in [`AdapterSlot`] (a sized struct holding the trait object)
    /// because `arc-swap 1.9.1`'s `RefCnt` for `Arc<T>` requires `T: Sized`
    /// — it does not support `ArcSwap<Arc<dyn Trait>>` directly.
    pub(crate) adapter: Arc<ArcSwap<AdapterSlot>>,
    /// Full-scan rebuild counter (V2.2 / #401 instrumentation). Incremented
    /// on EVERY fallback to a data-store-scan rebuild (no snapshot, corrupt
    /// snapshot, version mismatch); NOT incremented on a successful snapshot
    /// load. Exposed via [`rebuild_count`] so tests can prove the snapshot
    /// path was taken.
    full_rebuild_count: AtomicU64,
    /// V2.3 (#402) — number of vector mutations (upserts + deletes) appended
    /// to the delta-log since the last successful generation flip. Bumped by
    /// `append_vector_delta` (Phase 5d) and reset to 0 by the background
    /// snapshot task after a flip lands. Compared against the tunable
    /// threshold to decide whether to spawn the dump + flip + prune.
    /// Relaxed ordering is sufficient: a missed threshold crossing on one
    /// ack is harmless (the next ack re-checks); the only invariant is
    /// monotonic growth between flips, which `fetch_add` guarantees.
    ///
    /// `Arc`-shared so the single-flight background snapshot task can reset
    /// it after a successful flip without holding a borrow on the backend
    /// (the task is `'static`, and an inline `AtomicU64` cannot move into
    /// a spawned task).
    delta_count: Arc<AtomicU64>,
    /// V2.3 (#402) — single-flight guard for the background snapshot task.
    /// `compare_exchange(false, true)` on the threshold crossing arms the
    /// guard; the spawned task clears it (`store(false)`) on completion
    /// (success OR failure). A second crossing while the first task is in
    /// flight is a no-op — the counter keeps climbing and the next ack
    /// re-arms once the in-flight task clears the flag.
    ///
    /// `Arc`-shared for the same reason as `delta_count`.
    snapshot_in_flight: Arc<AtomicBool>,
    /// V2.3 (#402) — monotonic index of the next delta chunk to write.
    /// Seeded by `restore_on_open` from `highest_delta_index` so a restart
    /// does not collide with existing chunks; bumped by
    /// `append_vector_delta` on every chunk write. Touched only on the
    /// commit-ack path (Phase 5d is serial per tx) so a plain `fetch_add`
    /// is correct without a lock.
    ///
    /// `Arc`-shared so the background snapshot task can read it to compute
    /// the `delta_applied_upto` for the new manifest.
    next_delta_idx: Arc<AtomicU64>,
    /// Delta-count threshold that arms a background snapshot. Defaults to the
    /// production tunable `VECTOR_SNAPSHOT_DELTA_THRESHOLD`; overridable in
    /// tests (via `set_snapshot_threshold_for_test`) so the full
    /// trigger → spawn → `run_background_snapshot` path can be exercised
    /// deterministically without appending 10k real vectors.
    snapshot_threshold: AtomicU64,
}

impl VectorBackend {
    pub fn new(
        descriptor: IndexDescriptor,
        field_path: Vec<u64>,
        adapter: Arc<dyn VectorAdapter>,
    ) -> Self {
        Self {
            descriptor,
            field_path,
            adapter: Arc::new(ArcSwap::from(Arc::new(AdapterSlot { adapter }))),
            full_rebuild_count: AtomicU64::new(0),
            delta_count: Arc::new(AtomicU64::new(0)),
            snapshot_in_flight: Arc::new(AtomicBool::new(false)),
            next_delta_idx: Arc::new(AtomicU64::new(0)),
            snapshot_threshold: AtomicU64::new(VECTOR_SNAPSHOT_DELTA_THRESHOLD),
        }
    }

    /// Lower the background-snapshot threshold so a test can cross it without
    /// appending the production default (10k) vectors. Test-only.
    #[cfg(test)]
    pub(crate) fn set_snapshot_threshold_for_test(&self, threshold: u64) {
        self.snapshot_threshold.store(threshold, Ordering::Release);
    }

    /// Read the single-flight guard flag. Test-only — used to prove the flag is
    /// cleared after a background snapshot completes (drop-guard reset).
    #[cfg(test)]
    pub(crate) fn snapshot_in_flight_for_test(&self) -> bool {
        self.snapshot_in_flight.load(Ordering::Acquire)
    }

    /// Snapshot keyspace for this backend's index: `__vec_snap__<id>`.
    /// Derived from the persisted, stable `IndexDescriptor::id` so the
    /// keyspace survives index renames and re-derives identically on every
    /// open of the same index.
    fn snapshot_keyspace(&self) -> String {
        format!("{}{}", SNAPSHOT_KEYSPACE_PREFIX, self.descriptor.id)
    }

    /// Resolve `self.field_path` to its interned-key form.
    fn ipath(&self) -> SmallVec<[InternerKey; 4]> {
        self.field_path
            .iter()
            .map(|&id| InternerKey::new(id))
            .collect()
    }

    /// Extract the embedding vector from `rec` via `any_seq_elem`.
    ///
    /// Parity with the legacy manual `InnerValue::List` walk: returns
    /// `Some(Vec<f32>)` iff the field at `ipath` is a sequence whose
    /// elements are ALL numeric (F64/Int); returns `None` otherwise
    /// (non-sequence leaf, or any non-numeric scalar element).
    ///
    /// **Widening note vs. legacy:** `any_seq_elem` accepts List OR Set
    /// (the legacy code only accepted List). Vector fields are always
    /// List in practice, so this is harmless. Additionally,
    /// `any_seq_elem` silently skips non-scalar elements (nested
    /// Map/List/Set, Dec, Big) rather than reporting them to the
    /// callback; a non-scalar element in a vector field would be
    /// silently omitted rather than causing `None`. This never occurs
    /// with well-formed vector data (lists of numbers) and matches the
    /// plan's documented behaviour.
    fn extract_vec(&self, rec: &dyn RecordRef) -> Option<Vec<f32>> {
        let ipath = self.ipath();
        let mut v: Vec<f32> = Vec::with_capacity(4);
        let mut bad = false;
        let is_seq = rec.any_seq_elem(&ipath, &mut |sr| {
            match sr {
                ScalarRef::F64(f) => v.push(f as f32),
                ScalarRef::Int(n) => v.push(n as f32),
                _ => {
                    bad = true;
                    return true; // short-circuit any_seq_elem
                }
            }
            false // keep going
        });
        if is_seq.is_some() && !bad {
            Some(v)
        } else {
            None
        }
    }
}

fn ve(e: VectorError) -> IndexError {
    IndexError::Backend(e.to_string())
}

#[async_trait]
impl IndexBackend for VectorBackend {
    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    // HNSW mutation goes through the adapter. The non-tx variants
    // (`plan_insert` / `plan_update` / `plan_delete`) commit to the
    // live HNSW graph immediately. The tx-aware overrides below
    // (`plan_insert_tx` / `plan_update_tx` / `plan_delete_tx`) do NOT
    // touch the live graph when `tx_id == Some`: the executor instead
    // extracts the vector via [`staged_vector`] and buffers it in
    // `TxContext::staged_vectors`, so a rolled-back tx leaves no ghost
    // vectors on the live graph (HIGH-6).
    //
    // HIGH-6 (resolved, fully RAII): staged vectors live inside the
    // `TxContext` (tx-local state), not on the adapter. The commit
    // pipeline (`commit::commit_tx` Phase 5d) calls
    // `apply_staged_vectors` under the commit lock to promote the tx's
    // staged vectors into the live graph. Abort needs no counterpart —
    // a dropped tx discards `staged_vectors` by RAII. Non-tx queries
    // still see only the committed graph (see `HnswAdapter::search`).

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if let Some(v) = self.extract_vec(rec) {
            self.adapter
                .load_full()
                .adapter
                .upsert(rid, &v)
                .await
                .map_err(ve)?;
        }
        Ok(Vec::new())
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        _old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if let Some(v) = self.extract_vec(new) {
            self.adapter
                .load_full()
                .adapter
                .upsert(rid, &v)
                .await
                .map_err(ve)?;
        } else {
            self.adapter
                .load_full()
                .adapter
                .delete(rid)
                .await
                .map_err(ve)?;
        }
        Ok(Vec::new())
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        _rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        self.adapter
            .load_full()
            .adapter
            .delete(rid)
            .await
            .map_err(ve)?;
        Ok(Vec::new())
    }

    /// tx-aware insert planning (HIGH-6).
    ///
    /// `tx_id == Some` → no-op here: the live HNSW graph is left
    /// untouched. The executor stages the vector itself via
    /// [`staged_vector`] → `TxContext::stage_vector`, so a dropped
    /// (rolled-back) tx leaves no trace (RAII).
    ///
    /// `tx_id == None` → forwards to [`plan_insert`] (immediate
    /// commit to the live graph).
    async fn plan_insert_tx(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
        tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if tx_id.is_none() {
            return self.plan_insert(rid, rec).await;
        }
        Ok(Vec::new())
    }

    /// tx-aware update planning (HIGH-6). See [`plan_insert_tx`].
    async fn plan_update_tx(
        &self,
        rid: RecordId,
        old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
        tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if tx_id.is_none() {
            return self.plan_update(rid, old, new).await;
        }
        Ok(Vec::new())
    }

    /// tx-aware delete planning (HIGH-6). See [`plan_insert_tx`].
    async fn plan_delete_tx(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
        tx_id: Option<shamir_tx::TxId>,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        if tx_id.is_none() {
            return self.plan_delete(rid, rec).await;
        }
        Ok(Vec::new())
    }

    /// HIGH-6: hand the executor this record's embedding so it can stage
    /// it tx-locally (`TxContext::stage_vector`). `None` when the record
    /// carries no vector at this backend's field path.
    async fn staged_vector(
        &self,
        _rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Option<Vec<f32>> {
        self.extract_vec(rec)
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Vector { vec, k, opts } => {
                let results = self
                    .adapter
                    .load_full()
                    .adapter
                    .search(&vec, k, opts, None)
                    .await
                    .map_err(ve)?;
                Ok(IndexResult::Ranked(results))
            }
            _ => Err(IndexError::Backend(
                "VectorBackend only supports Vector queries".into(),
            )),
        }
    }

    async fn lookup_tx(
        &self,
        _table_token: u64,
        query: IndexQuery,
        _tx: Option<&shamir_tx::TxContext>,
        staged_vectors: Option<&[(RecordId, Vec<f32>)]>,
    ) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Vector { vec, k, opts } => {
                let results = self
                    .adapter
                    .load_full()
                    .adapter
                    .search(&vec, k, opts, staged_vectors)
                    .await
                    .map_err(ve)?;
                Ok(IndexResult::Ranked(results))
            }
            _ => Err(IndexError::Backend(
                "VectorBackend only supports Vector queries".into(),
            )),
        }
    }

    /// HIGH-6: promote the tx's staged vectors for this table into the
    /// live HNSW graph at commit. Delegates to the adapter.
    async fn apply_staged_vectors(&self, vecs: &[(RecordId, Vec<f32>)]) -> Result<(), IndexError> {
        self.adapter
            .load_full()
            .adapter
            .apply_committed_vectors(vecs)
            .await
            .map_err(ve)
    }

    async fn rebuild(&self, source: Arc<dyn Store>) -> Result<(), IndexError> {
        // Count this full-scan rebuild for the V2.2 instrumentation.
        // `restore_on_open` skips `rebuild` entirely on a successful
        // snapshot load, so this counter ONLY moves when we genuinely
        // fell back to a scan.
        self.full_rebuild_count.fetch_add(1, Ordering::Relaxed);

        // Batch size = 1000: large enough that a single HnswAdapter
        // `upsert_batch` call drives a rayon `parallel_insert` over ~1k
        // vectors (well past the ~`1000 * num_threads` sweet spot the
        // hnsw_rs docstring recommends for parallel_insert efficiency),
        // yet bounded so a single batch's owned-Vec materialisation stays
        // in the low-MB range (1000 × 4-byte × dim). The store stream
        // already paginates at this granularity, so we hand each page
        // straight to the adapter as one batch.
        let batch_size = 1000usize;
        let mut stream = source.iter_stream(batch_size);
        while let Some(batch_res) = stream.next().await {
            let batch = batch_res.map_err(|e| IndexError::Storage(e.to_string()))?;
            let mut items: Vec<(RecordId, Vec<f32>)> = Vec::new();
            for (key_bytes, val_bytes) in batch {
                let arr: [u8; 16] = key_bytes
                    .as_ref()
                    .try_into()
                    .map_err(|_| IndexError::Backend("invalid key length".into()))?;
                let rid = RecordId(arr);
                let rec = InnerValue::from_bytes(&val_bytes)
                    .map_err(|e| IndexError::Backend(e.to_string()))?;
                if let Some(v) = self.extract_vec(&rec as &dyn RecordRef) {
                    items.push((rid, v));
                }
            }
            // One batched upsert per store page: HnswAdapter overrides
            // `upsert_batch` → single rayon parallel_insert over the page
            // (was N serial inserts in 64-wide inner chunks). The default
            // `upsert_batch` (BruteForceAdapter etc.) falls back to a
            // per-row loop, so the call is correct for every adapter.
            if !items.is_empty() {
                self.adapter
                    .load_full()
                    .adapter
                    .upsert_batch(&items)
                    .await
                    .map_err(ve)?;
            }
        }
        Ok(())
    }

    /// V2.2 / #401 — startup restore with snapshot-first, rebuild-fallback.
    /// V2.3 / #402 — the snapshot-hit branch ALSO replays the delta-log.
    ///
    /// Three branches:
    /// 1. **`Ok(adapter)`** — the snapshot loaded cleanly. We swap the
    ///    freshly-built `HnswAdapter` into the live `ArcSwap`, replacing
    ///    the empty placeholder adapter that `build_index2_backend_*`
    ///    constructed. NO data-store scan runs, so
    ///    [`rebuild_count`] stays at 0. This is the O(load) fast path.
    ///    **V2.3:** after the swap, we replay every delta chunk with index
    ///    > `manifest.delta_applied_upto` against the freshly-loaded adapter
    ///    so the graph reflects every mutation committed since the snapshot
    ///    was taken. The in-memory HWM (`next_delta_idx`) is seeded from the
    ///    highest chunk index in the store so the next `append_delta` does
    ///    not collide.
    /// 2. **`Err(NotFound)`** — no snapshot exists for this keyspace (a
    ///    fresh index, or the first open after the feature shipped). Fall
    ///    back to the legacy full-scan [`rebuild`] against the data store.
    ///    The rebuild counter increments to 1.
    /// 3. **`Err(Corrupt | VersionMismatch | Io | Backend | Serde)`** — the
    ///    snapshot exists but is unusable (bit-flip, format bump, foreign
    ///    `dim`/`metric`). We `log::warn!` the cause and fall back to a
    ///    full-scan [`rebuild`]; the user's data in the data store is
    ///    intact, so the scan rebuilds a correct graph. We do NOT abort
    ///    the open: a missing/stale snapshot is recoverable, and aborting
    ///    would make the whole table unopenable for a transient metadata
    ///    issue. The rebuild counter increments to 1.
    ///
    /// The snapshot keyspace is [`snapshot_keyspace`] — derived from the
    /// persisted `IndexDescriptor::id`, so it is stable across opens of
    /// the same index and immune to renames. `info_store` is where V2.1's
    /// `dump_snapshot` wrote the chunks/sidecar/manifest; it is the SAME
    /// store the engine hands every index2 backend at construction time
    /// (`build_index2_backend_with_resolver`'s `info_store` arg).
    async fn restore_on_open(
        &self,
        info_store: Arc<dyn Store>,
        data_store: Arc<dyn Store>,
    ) -> Result<(), IndexError> {
        let keyspace = self.snapshot_keyspace();
        // Read the manifest UP FRONT so we know `delta_applied_upto` for the
        // replay even if `load_snapshot` later swaps the adapter in. A
        // missing manifest is the `NotFound` branch — same as load_snapshot.
        let manifest_opt: Option<SnapshotManifest> = match snapshot::read_manifest(
            &info_store,
            &keyspace,
        )
        .await
        {
            Ok(m) => Some(m),
            Err(SnapshotError::NotFound) => None,
            Err(e) => {
                // A corrupt manifest is treated the same as a corrupt
                // snapshot below: warn + fall back to a full rebuild.
                log::warn!(
                        "vector manifest read failed for index {} (keyspace {}): {} — falling back to full rebuild",
                        self.descriptor.name,
                        keyspace,
                        e
                    );
                None
            }
        };
        match snapshot::load_snapshot(&info_store, &keyspace).await {
            Ok(loaded) => {
                // O(load) fast path: hand the rebuilt adapter to the live
                // backend. `ArcSwap::store` is wait-free; a concurrent query
                // grabbing `load()` either sees the old empty adapter or the
                // new one — never a torn state.
                let adapter_arc: Arc<dyn VectorAdapter> = Arc::new(loaded);
                self.adapter.store(Arc::new(AdapterSlot {
                    adapter: Arc::clone(&adapter_arc),
                }));

                // V2.3: replay delta chunks past the snapshot's base. The
                // `delta_applied_upto` comes from the manifest we just read
                // (load_snapshot validated it but does not return it).
                if let Some(manifest) = manifest_opt {
                    let delta_upto = manifest.delta_applied_upto;
                    if let Some(hnsw) = adapter_arc.as_hnsw_adapter() {
                        if let Err(e) =
                            snapshot::replay_delta(&info_store, &keyspace, delta_upto, hnsw).await
                        {
                            // A delta-replay failure is recoverable: the base
                            // graph is intact, only the post-snapshot
                            // mutations were lost. Warn and continue — the
                            // next snapshot flip will capture a fresh base.
                            log::warn!(
                                "vector delta replay failed for index {} (keyspace {}): {} — \
                                 base snapshot loaded, post-snapshot mutations may be missing \
                                 until the next snapshot",
                                self.descriptor.name,
                                keyspace,
                                e
                            );
                        }
                    }
                }

                // Seed the in-memory delta HWM so the next `append_delta`
                // does not collide with an existing chunk. `highest_delta_index`
                // returns 0 when no chunks exist (fresh snapshot).
                let hwm = snapshot::highest_delta_index(&info_store, &keyspace)
                    .await
                    .unwrap_or(0);
                self.next_delta_idx
                    .store(hwm.saturating_add(1), Ordering::Release);

                Ok(())
            }
            Err(SnapshotError::NotFound) => {
                // No snapshot for this index — fresh index or first open.
                // Fall through to a full rebuild scan.
                self.rebuild(data_store).await
            }
            Err(e) => {
                // Snapshot present but unusable. Warn loudly (this is the
                // signal an operator uses to investigate a failing snapshot)
                // and recover via a full rebuild scan. We do NOT propagate
                // the snapshot error: the data store is the source of truth
                // and a rebuild will reconstruct a correct graph.
                log::warn!(
                    "vector snapshot load failed for index {} (keyspace {}): {} — falling back to full rebuild",
                    self.descriptor.name,
                    keyspace,
                    e
                );
                self.rebuild(data_store).await
            }
        }
    }

    fn rebuild_count(&self) -> u64 {
        self.full_rebuild_count.load(Ordering::Relaxed)
    }

    async fn drop_all(&self) -> Result<(), IndexError> {
        // Adapter doesn't have a "clear" method — for now noop.
        // Full impl would iterate all rids and delete.
        Ok(())
    }

    /// V2.3 (#402) — append a delta-log chunk capturing the vectors just
    /// promoted into the live graph, then bump the mutation counter.
    ///
    /// See [`IndexBackend::append_vector_delta`] for the contract. The chunk
    /// is written at the monotonic `next_delta_idx` (one `Store::set`); the
    /// HWM is bumped AFTER the write lands so a crash between the write and
    /// the bump leaves a duplicate-index chunk on the next attempt (which
    // `Store::set` overwrites — last-writer-wins on the chunk key, and the
    // ops are idempotent under replay because upsert/delete are
    // last-write-wins on the adapter).
    async fn append_vector_delta(
        &self,
        info_store: &Arc<dyn Store>,
        vecs: &[(RecordId, Vec<f32>)],
        deleted: &[RecordId],
    ) -> Result<(), IndexError> {
        // Build the chunk. An empty promote (no vecs, no deletes) writes
        // nothing — keeps the chunk stream dense.
        if vecs.is_empty() && deleted.is_empty() {
            return Ok(());
        }
        let mut ops: Vec<snapshot::DeltaOp> = Vec::with_capacity(vecs.len() + deleted.len());
        for (rid, vec) in vecs {
            ops.push(snapshot::DeltaOp::Upsert(*rid, vec.clone()));
        }
        for rid in deleted {
            ops.push(snapshot::DeltaOp::Delete(*rid));
        }
        let keyspace = self.snapshot_keyspace();
        let idx = self.next_delta_idx.fetch_add(1, Ordering::AcqRel);
        snapshot::append_delta(info_store, &keyspace, idx, &ops)
            .await
            .map_err(|e| IndexError::Backend(format!("delta append: {e}")))?;
        // Bump the mutation counter by the number of ops in this chunk. The
        // background snapshot trigger reads this counter to decide when to
        // flip the generation.
        self.delta_count
            .fetch_add(ops.len() as u64, Ordering::AcqRel);
        Ok(())
    }

    /// V2.3 (#402) — check the delta counter against the threshold and, if
    /// crossed, spawn a single-flight background snapshot task.
    ///
    /// See [`IndexBackend::trigger_snapshot_check`] for the contract. The
    /// task is a `tokio::spawn` (NOT awaited) so the commit-ack path returns
    /// immediately (§5.6). The single-flight `AtomicBool` prevents two
    /// concurrent dumps from racing on the same keyspace.
    fn trigger_snapshot_check(&self, info_store: &Arc<dyn Store>) {
        let count = self.delta_count.load(Ordering::Acquire);
        if count < self.snapshot_threshold.load(Ordering::Acquire) {
            return;
        }
        // Single-flight: a crossing while a dump is already running is a
        // no-op. The counter keeps climbing and the next ack re-arms once
        // the in-flight task clears the flag.
        if self
            .snapshot_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        // Capture the CURRENT adapter (an `Arc<dyn VectorAdapter>` inside an
        // `AdapterSlot`) by cloning the slot's Arc. The spawned task holds
        // this Arc for the duration of the dump; a concurrent
        // `restore_on_open` swap would install a NEW slot, but the task's
        // dump reflects the adapter state at capture time — correct, because
        // the dump is a point-in-time snapshot of the graph.
        let adapter_arc = Arc::clone(&self.adapter.load_full().adapter);
        // The background snapshot needs the concrete HnswAdapter. If the
        // current adapter is not an HnswAdapter (BruteForce, future external
        // adapters), there is no snapshot to take — clear the flag and
        // return without spawning.
        if adapter_arc.as_hnsw_adapter().is_none() {
            self.snapshot_in_flight.store(false, Ordering::Release);
            return;
        }
        let keyspace = self.snapshot_keyspace();
        let info_store = Arc::clone(info_store);
        let next_delta_idx = Arc::clone(&self.next_delta_idx);
        let delta_count = Arc::clone(&self.delta_count);
        // Move the flag into a drop-guard so it is cleared on EVERY exit of the
        // task — normal return, `Err`, OR panic (unwind still runs `Drop`). A
        // bare `store(false)` at the end would be skipped on panic, leaving the
        // flag stuck `true` forever → snapshots silently disabled for the life
        // of the process and the delta-log growing unbounded.
        let flight_guard = SnapshotFlightGuard(Arc::clone(&self.snapshot_in_flight));
        tokio::spawn(async move {
            let _flight_guard = flight_guard; // cleared on drop (incl. unwind)
            let res =
                run_background_snapshot(&info_store, &keyspace, &adapter_arc, &next_delta_idx)
                    .await;
            match res {
                Ok(()) => {
                    // Reset the mutation counter — the new snapshot absorbed
                    // every chunk up to the current HWM.
                    delta_count.store(0, Ordering::Release);
                }
                Err(e) => {
                    log::warn!(
                        "vector background snapshot failed for keyspace {}: {} — \
                         will retry on the next threshold crossing",
                        keyspace,
                        e
                    );
                }
            }
        });
    }
}

/// Resets the single-flight `snapshot_in_flight` flag on drop, so the flag is
/// cleared whether the background snapshot task returns `Ok`, returns `Err`, or
/// **panics** (unwinding still runs `Drop`). Without this, a panic inside the
/// dump would leak the flag as `true` permanently and disable all future
/// snapshots for the process (pillar: a guard must never stick).
struct SnapshotFlightGuard(Arc<AtomicBool>);

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
async fn run_background_snapshot(
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
        basename: written.basename,
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
