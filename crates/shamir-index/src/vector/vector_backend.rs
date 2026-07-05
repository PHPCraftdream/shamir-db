//! `IndexBackend` wrapper around any `VectorAdapter`.
//!
//! Extracts the vector field from records, delegates to the adapter
//! for similarity search. Returns `IndexResult::Ranked`.

use super::adapter::{VectorAdapter, VectorError};
use super::snapshot::{self, SnapshotError};
use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::write_ops::IndexWriteOp;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures::StreamExt;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::{RecordRef, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicU64, Ordering};
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
        }
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
            self.adapter.load_full().adapter.delete(rid).await.map_err(ve)?;
        }
        Ok(Vec::new())
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        _rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        self.adapter.load_full().adapter.delete(rid).await.map_err(ve)?;
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
    ///
    /// Three branches:
    /// 1. **`Ok(adapter)`** — the snapshot loaded cleanly. We swap the
    ///    freshly-built `HnswAdapter` into the live `ArcSwap`, replacing
    ///    the empty placeholder adapter that `build_index2_backend_*`
    ///    constructed. NO data-store scan runs, so
    ///    [`rebuild_count`] stays at 0. This is the O(load) fast path.
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
        match snapshot::load_snapshot(&info_store, &keyspace).await {
            Ok(loaded) => {
                // O(load) fast path: hand the rebuilt adapter to the live
                // backend. `ArcSwap::store` is wait-free; a concurrent query
                // grabbing `load()` either sees the old empty adapter or the
                // new one — never a torn state.
                self.adapter.store(Arc::new(AdapterSlot {
                    adapter: Arc::new(loaded),
                }));
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
}
