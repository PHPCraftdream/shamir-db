//! Sorted (B-tree-by-value) index manager.
//!
//! Parallel to the hash-based `IndexManager`. Where hash indexes
//! answer **equality** lookups (`field == value`), sorted indexes
//! answer **range / order / min** queries by encoding the indexed
//! value into bytes that sort the same way the value does (see
//! `shamir_types::core::sort_codec`) and storing one info-store
//! record per `(value, record_id)` pair.
//!
//! What's supported in this first cut:
//! - Single-field index over a scalar column (Int / Float / String /
//!   Bool / U64).
//! - Range queries: between / gt / gte / lt / lte.
//! - `order by field asc + limit K` (forward scan, stop after K).
//! - `min(field)` (first record from prefix scan).
//!
//! Not yet:
//! - `max(field)`, `order by desc` — needs reverse iteration on the
//!   Store trait (next).
//! - Composite sorted index over multiple columns.

use bytes::Bytes;
use futures::StreamExt;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::sync::Arc;
// Re-export so existing callers that import `SortedIndexDefinition` from this
// module continue to compile unchanged after the type moved to its own file.
pub use crate::legacy::sorted_index_definition::SortedIndexDefinition;
use crate::legacy::sorted_index_definition::{SortedIndexDefinitionV1, SORTED_TAG};
use crate::write_ops::IndexWriteOp;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::core::sort_codec;
use shamir_types::record_view::RecordRef;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

/// Manages a set of sorted indexes for one table.
///
/// # Storage
///
/// Definitions live in a `NodeReplicated<Vec<SortedIndexDefinition>>` — a
/// NUMA-aware, read-mostly RCU-style snapshot. Reads (`iter_indexes`/
/// `has_indexes`/`find_by_*`/`has_covering_indexes`) are lock-free against
/// the calling thread's node-local `Arc<Vec<...>>` replica; writes
/// (`register`/`drop_index`/`rename_definition`/`intern_included_paths`)
/// copy-on-write via `NodeReplicated::rcu` and mirror to all per-node
/// replicas. On single-socket machines (dev, Windows, CI) there is exactly
/// one replica, giving identical performance to a bare `ArcSwap`. On
/// multi-socket NUMA machines each node reads its own replica without
/// crossing a socket interconnect.
///
/// Replaces the previous sharded `DashMap` whose per-shard read-locks
/// fired on every `plan_record_*` (singular insert path) and every
/// batch — mirrors the refactor that `IndexInfo` got in N3.
///
/// # Persistence
///
/// Persisted as a single system record under
/// `RecordId::system("sorted_indexes")` so we can reload on restart.
///
/// # Cardinality assumption
///
/// Typical workloads have ≤ ~10 sorted indexes per table — linear scan
/// over the Vec is cache-friendly and beats DashMap shard locks; matches
/// N3's profile of IndexInfo.
pub struct SortedIndexManager {
    info_store: Arc<dyn Store>,
    /// NUMA-aware RCU snapshot of `Vec<SortedIndexDefinition>`.
    /// Each NUMA node owns its own cache-padded replica so reads never
    /// cross a socket interconnect. Writes copy-on-write on node 0 and
    /// mirror to all other nodes.
    ///
    /// **Shared across clones via `Arc`.** `TableManager` (and hence this
    /// manager) is cloned on every `get_table()` — the DDL path
    /// (`create_sorted_index*` → `register`) and the read path
    /// (`iter_indexes`) each hold their own `TableManager` clone. A
    /// per-clone `NodeReplicated` (the previous design) desynced: a
    /// `register` on the DDL clone COW-updated only that clone's replicas,
    /// so the next read clone (snapshotted from the OnceCell primary, whose
    /// replicas were never touched) saw zero indexes. Wrapping in `Arc`
    /// makes every clone observe the same `NodeReplicated`, so any clone's
    /// `register`/`drop_index`/`rename`/`intern_included_paths` is visible
    /// to every other clone — mirroring how the sibling `IndexManager`
    /// shares its `Arc<IndexInfo>` and how `TableManager` shares
    /// `bindings_len`/`validator_bindings` through `Arc`.
    indexes: Arc<shamir_numa::NodeReplicated<Vec<SortedIndexDefinition>>>,
}

impl Clone for SortedIndexManager {
    fn clone(&self) -> Self {
        // Share the SAME NodeReplicated across clones so a register/drop on
        // any clone is visible to every other clone (see the field doc for
        // the read-after-write desync this prevents). A snapshot-copy here
        // would silently drop DDL-registered indexes from later read clones.
        Self {
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
        }
    }
}

impl SortedIndexManager {
    /// Construct empty; caller must `load()` to hydrate.
    pub async fn new(info_store: Arc<dyn Store>) -> DbResult<Self> {
        let m = Self {
            info_store,
            indexes: Arc::new(shamir_numa::NodeReplicated::new(
                shamir_numa::detect(),
                Vec::new(),
            )),
        };
        m.load().await?;
        Ok(m)
    }

    /// True if at least one sorted index exists.
    pub fn has_indexes(&self) -> bool {
        !self.indexes.load_local().is_empty()
    }

    /// True if at least one sorted index has non-empty `included_fields`
    /// (i.e. is a covering index). Used to skip early interner
    /// initialization on open when no covering projections are needed.
    pub fn has_covering_indexes(&self) -> bool {
        self.indexes
            .load_local()
            .iter()
            .any(|d| !d.included_fields.is_empty())
    }

    /// Iterate over all sorted-index definitions.
    pub fn iter_indexes(&self) -> Vec<SortedIndexDefinition> {
        // Snapshot the current node-local Arc<Vec<...>> and clone its contents.
        // load_local() → Guard<Arc<T>>; *guard → Arc<T>; **guard → Vec<...>.
        // Callers consume by-value; for hot-path planners that just
        // need a borrow, see future `snapshot()` accessor.
        (**self.indexes.load_local()).clone()
    }

    /// Look up a definition whose `field_path` matches.
    pub fn find_by_field(&self, field_path: &[u64]) -> Option<SortedIndexDefinition> {
        self.indexes
            .load_local()
            .iter()
            .find(|d| d.field_path == field_path)
            .cloned()
    }

    /// Look up a definition by its interned name id.
    /// Used by the index-only read path (slice A3) to check
    /// whether the scanned index is a covering index.
    pub fn find_by_name_interned(&self, name_interned: u64) -> Option<SortedIndexDefinition> {
        self.indexes
            .load_local()
            .iter()
            .find(|d| d.name_interned == name_interned)
            .cloned()
    }

    /// Register a new sorted index (copy-on-write under a CAS loop).
    /// Persists the updated definitions blob, but does NOT backfill —
    /// the caller scans the table and calls `insert_entry` for each
    /// existing record.
    ///
    /// Last-write-wins matches the previous `DashMap::insert` semantics:
    /// if a definition with the same `name_interned` exists, it is
    /// replaced in-place; otherwise appended.
    pub async fn register(&self, def: SortedIndexDefinition) -> DbResult<()> {
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            match new_vec
                .iter()
                .position(|d| d.name_interned == def.name_interned)
            {
                Some(pos) => new_vec[pos] = def.clone(),
                None => new_vec.push(def.clone()),
            }
            new_vec
        });
        self.persist_defs().await
    }

    /// Drop a sorted index definition AND every entry written under
    /// it. O(I) where I is the size of the index.
    pub async fn drop_index(&self, name_interned: u64) -> DbResult<bool> {
        let mut existed = false;
        self.indexes.rcu(|cur| {
            let initial_len = cur.len();
            let new_vec: Vec<SortedIndexDefinition> = cur
                .iter()
                .filter(|d| d.name_interned != name_interned)
                .cloned()
                .collect();
            existed = new_vec.len() != initial_len;
            new_vec
        });
        if !existed {
            return Ok(false);
        }
        // Sweep entries.
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);
        let mut to_drop: Vec<Bytes> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                to_drop.push(k);
            }
        }
        if !to_drop.is_empty() {
            // Ok-value (removed entries) intentionally discarded; ? propagates errors.
            let _ = self.info_store.remove_many(to_drop).await?;
        }
        self.persist_defs().await?;
        Ok(true)
    }

    /// Re-key an in-memory sorted-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// This is the metadata half of RENAME INDEX for sorted indexes — the
    /// physical posting entries are re-keyed separately by the engine
    /// (`rekey_sorted_prefix`). Here we only swap the in-memory entry and
    /// re-save the definitions blob.
    ///
    /// Note: `drop_index` would delete the physical entries we just moved, so
    /// we bypass it and manipulate the `indexes` snapshot directly via `rcu`.
    pub async fn rename_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let mut not_found = false;
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            match new_vec.iter().position(|d| d.name_interned == old_id) {
                Some(pos) => {
                    new_vec[pos].name_interned = new_id;
                    not_found = false;
                }
                None => {
                    not_found = true;
                }
            }
            new_vec
        });
        if not_found {
            return Err(shamir_storage::error::DbError::Internal(
                "sorted index definition disappeared mid-rename".to_string(),
            ));
        }
        self.persist_defs().await
    }

    // ============================================================================
    // Covering-index helpers
    // ============================================================================

    /// Resolve `included_fields` string paths to interned u64 ids for every
    /// definition that has at least one included field. Call this:
    ///   1. After `register()` when the caller already has an interner, OR
    ///   2. After construction (load from disk) to rebuild the transient
    ///      `included_fields_interned` caches.
    ///
    /// Unknown strings are silently skipped (they produce an empty inner vec
    /// for that path, which `build_covering_projection` will treat as absent).
    pub fn intern_included_paths(&self, interner: &Interner) {
        // Single COW pass: clone the current snapshot, mutate each def
        // in-place, store the new Arc. Replaces the previous per-key
        // DashMap::alter loop. Off hot path (called after register with
        // interner OR after load on bootstrap) so the one-shot Vec clone
        // is acceptable.
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            for def in new_vec.iter_mut() {
                if def.included_fields.is_empty() {
                    continue;
                }
                def.included_fields_interned = def
                    .included_fields
                    .iter()
                    .map(|path_segs| {
                        path_segs
                            .iter()
                            .filter_map(|seg| {
                                interner.touch_ind(seg.as_str()).ok().map(|t| t.key().id())
                            })
                            .collect::<Vec<u64>>()
                    })
                    .collect();
            }
            new_vec
        });
    }

    // ============================================================================
    // Planner methods — return Vec<IndexWriteOp> without side effects
    // ============================================================================

    /// Plan index entries for a newly created record.
    pub fn plan_record_created(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::with_capacity(4);
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                let value = if def.is_covering() {
                    build_covering_projection(record, def, version)
                } else {
                    Bytes::new()
                };
                ops.push(IndexWriteOp::SetPosting { key, value });
            }
        }
        Ok(ops)
    }

    /// Planner variant of [`on_records_created_batch`] — collects
    /// entry ops for N records across all sorted indexes in one
    /// pass, snapshotting `iter_indexes()` ONCE (the per-row
    /// `plan_record_created` re-snapshots every call). Used by the
    /// tx batch insert path.
    pub fn plan_records_created_batch<'a, R, I>(
        &self,
        items: I,
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    ops.push(IndexWriteOp::SetPosting { key, value: pv });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry changes when a record is updated.
    pub fn plan_record_updated(
        &self,
        record_id: &RecordId,
        old: &(impl RecordRef + ?Sized),
        new: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            let old_enc = extract_and_encode(old, &def.field_path)?;
            let new_enc = extract_and_encode(new, &def.field_path)?;
            // For covering indexes, also rewrite the posting when the
            // projected values changed even if the indexed key did not.
            // Both old and new are built with the same `version` so that
            // the version bytes are identical and do not spuriously trigger
            // a rewrite when only the version changed.
            let old_proj = if def.is_covering() {
                Some(build_covering_projection(old, def, version))
            } else {
                None
            };
            let new_proj = if def.is_covering() {
                Some(build_covering_projection(new, def, version))
            } else {
                None
            };
            let key_changed = old_enc != new_enc;
            let proj_changed = old_proj != new_proj;
            if !key_changed && !proj_changed {
                continue;
            }
            if key_changed {
                if let Some(ref ov) = old_enc {
                    let key = self.build_entry_key(def.name_interned, ov, record_id);
                    ops.push(IndexWriteOp::RemovePosting { key });
                }
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting { key, value });
                }
            } else {
                // Key is the same but projection changed — overwrite in place.
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting { key, value });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry removals for a deleted record.
    pub fn plan_record_deleted(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                ops.push(IndexWriteOp::RemovePosting { key });
            }
        }
        Ok(ops)
    }

    // ============================================================================
    // Apply ops
    // ============================================================================

    /// Apply a slice of `IndexWriteOp` against `self.info_store`.
    async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
        for op in ops {
            match op {
                IndexWriteOp::SetPosting { key, value } => {
                    self.info_store.set(key.clone(), value.clone()).await?;
                }
                IndexWriteOp::RemovePosting { key } => {
                    let _ = self.info_store.remove(key.clone()).await?;
                }
                IndexWriteOp::BumpFtsStats { .. } => {
                    // Not relevant for SortedIndexManager.
                }
            }
        }
        Ok(())
    }

    // ============================================================================
    // on_record_* wrappers — plan + apply
    // ============================================================================

    /// Add an index entry for a record. Called from
    /// `TableManager::insert` and `set` (create branch).
    pub async fn on_record_created(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_created(record_id, record, version)?;
        self.apply_ops(&ops).await
    }

    /// Update entries when a record changes.
    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old: &(impl RecordRef + ?Sized),
        new: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_updated(record_id, old, new, version)?;
        self.apply_ops(&ops).await
    }

    /// Batched version of `on_record_created` — collects all entry
    /// writes across all sorted indexes for N records into one
    /// `Store::set_many` call. Borrow-only — no `InnerValue` clones
    /// except for covering-index projection (unavoidable: projection
    /// requires a deep clone of the leaf value).
    pub async fn on_records_created_batch<'a, R, I>(&self, items: I, version: u64) -> DbResult<()>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if self.indexes.load_local().is_empty() {
            return Ok(());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut writes: Vec<(Bytes, Bytes)> = Vec::new();
        for def in &defs {
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    writes.push((key, pv));
                }
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        self.info_store.set_many(writes).await?;
        Ok(())
    }

    /// Drop entries for a deleted record.
    pub async fn on_record_deleted(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
    ) -> DbResult<()> {
        let ops = self.plan_record_deleted(record_id, record)?;
        self.apply_ops(&ops).await
    }

    /// Range lookup: return all record IDs whose indexed value is in
    /// `[start, end]` (both inclusive). `start` / `end` are the
    /// already-encoded value bytes (call sites use
    /// `sort_codec::encode_*` to produce them).
    ///
    /// Builds the lower / upper bounds in the physical-key space and
    /// delegates to `Store::iter_range_stream` — on B-tree-backed
    /// stores (sled, redb, fjall, persy, canopy) this seeks straight
    /// to `lower` and stops at `upper`, doing zero wasted work
    /// outside the range. In-memory / cached fall back to
    /// `iter_range_stream`'s default filter wrapper, still correct.
    pub async fn lookup_range(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<BTreeSet<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);

        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut out: BTreeSet<RecordId> = BTreeSet::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                if let Some(id) = decode_record_id_suffix(k.as_ref()) {
                    out.insert(id);
                }
            }
        }
        Ok(out)
    }

    /// Range lookup with physical values: identical to [`lookup_range`] but
    /// returns `(RecordId, Bytes)` pairs (preserving scan / value order in a
    /// `Vec`, NOT de-duplicating into a `BTreeSet`).  The `Bytes` is the
    /// raw physical_value stored in the index entry — for covering indexes
    /// that is the versioned projection envelope written by
    /// `build_covering_projection`; for non-covering indexes it is empty.
    ///
    /// Used by the index-only read path (slice A3).
    pub async fn lookup_range_with_values(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<Vec<(RecordId, Bytes)>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);

        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut out: Vec<(RecordId, Bytes)> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                if let Some(id) = decode_record_id_suffix(k.as_ref()) {
                    out.push((id, v));
                }
            }
        }
        Ok(out)
    }

    /// Min lookup — the first record under the sorted prefix.
    /// `iter_range_stream` with batch_size=1 reads exactly the first
    /// entry on B-tree backends; in-memory falls back to its default.
    pub async fn lookup_min(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            if let Some((k, _)) = batch?.into_iter().next() {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// Max lookup — the last record under the sorted prefix.
    /// Uses `iter_range_stream_reverse` so disk backends seek
    /// straight to the upper bound and walk one entry backwards.
    pub async fn lookup_max(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream = self
            .info_store
            .iter_range_stream_reverse(Some(lower), Some(upper), 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            if let Some((k, _)) = batch?.into_iter().next() {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// Last K record ids under the sorted prefix, in value-DESC order.
    /// Mirror of `lookup_first_k` using `iter_range_stream_reverse`.
    pub async fn lookup_last_k(&self, name_interned: u64, k: usize) -> DbResult<Vec<RecordId>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream =
            self.info_store
                .iter_range_stream_reverse(Some(lower), Some(upper), k.min(256));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// First K record ids under the sorted prefix, in value-asc order.
    pub async fn lookup_first_k(&self, name_interned: u64, k: usize) -> DbResult<Vec<RecordId>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream =
            self.info_store
                .iter_range_stream(Some(lower), Some(upper), k.min(MAINT_SCAN_BATCH));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// tx-aware variant of [`lookup_range`].
    ///
    /// Phase C (Step 5): records an `IndexRange` predicate dependency
    /// for Serializable txs BEFORE forwarding to the non-tx method.
    /// Zero-overhead: Snapshot / non-tx callers skip the recording
    /// block entirely (single tag-compare on `Option<&TxContext>`).
    pub async fn lookup_range_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<BTreeSet<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_range(name_interned, start_encoded, end_encoded)
            .await
    }

    /// tx-aware variant of [`lookup_min`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency (the entire sorted index) for Serializable txs.
    pub async fn lookup_min_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_min(name_interned).await
    }

    /// tx-aware variant of [`lookup_max`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs.
    pub async fn lookup_max_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_max(name_interned).await
    }

    /// tx-aware variant of [`lookup_last_k`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs. The interval does not depend on
    /// `k` — every entry the scan could reach is in the full-prefix range.
    pub async fn lookup_last_k_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        k: usize,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_last_k(name_interned, k).await
    }

    /// tx-aware variant of [`lookup_first_k`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs.
    pub async fn lookup_first_k_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        k: usize,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_first_k(name_interned, k).await
    }

    /// Build inclusive (lower, upper) physical-key bounds for one
    /// sorted-index range query.
    ///
    /// - `start_encoded = None` → lower = `prefix` itself (start of
    ///   the index's keyspace).
    /// - `end_encoded = None` → upper = `prefix || [0xFF; 64]`,
    ///   strictly greater than any real entry in this prefix and
    ///   strictly less than the start of the next prefix
    ///   (`name_interned + 1`), so it correctly bounds "everything in
    ///   this index" without leaking into neighbours.
    /// - Otherwise the bounds are `prefix || encoded[ || 0xFF×16]`.
    fn range_bounds(
        &self,
        prefix: &Bytes,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> (Bytes, Bytes) {
        let lower = match start_encoded {
            Some(enc) => {
                let mut k = Vec::with_capacity(prefix.len() + enc.len());
                k.extend_from_slice(prefix);
                k.extend_from_slice(enc);
                Bytes::from(k)
            }
            None => prefix.clone(),
        };
        let upper = match end_encoded {
            Some(enc) => {
                let mut k = Vec::with_capacity(prefix.len() + enc.len() + 16);
                k.extend_from_slice(prefix);
                k.extend_from_slice(enc);
                // Cover all record_id tiebreakers at the upper value.
                k.extend_from_slice(&[0xFFu8; 16]);
                Bytes::from(k)
            }
            None => {
                let mut k = Vec::with_capacity(prefix.len() + 64);
                k.extend_from_slice(prefix);
                k.extend_from_slice(&[0xFFu8; 64]);
                Bytes::from(k)
            }
        };
        (lower, upper)
    }

    // ----- internals --------------------------------------------------------

    /// Count of entries currently in the sorted index — used by the
    /// doctor's verify pass. O(K) where K is the entry count.
    pub async fn entry_count(&self, name_interned: u64) -> DbResult<u64> {
        let prefix = self.entry_prefix(name_interned);
        let mut count: u64 = 0;
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        Ok(count)
    }

    /// True iff `record` carries a value at `field_path` that the
    /// sort codec can encode (i.e. an entry for this record *should*
    /// exist in a sorted index keyed on this path).
    pub fn has_indexable_value(record: &(impl RecordRef + ?Sized), field_path: &[u64]) -> bool {
        matches!(extract_and_encode(record, field_path), Ok(Some(_)))
    }

    /// Prefix common to every entry of one sorted index.
    fn entry_prefix(&self, name_interned: u64) -> Bytes {
        let mut buf = Vec::with_capacity(9);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        Bytes::from(buf)
    }

    /// Full entry key for one (value, record_id) pair.
    fn build_entry_key(
        &self,
        name_interned: u64,
        encoded_value: &[u8],
        record_id: &RecordId,
    ) -> Bytes {
        let mut buf = Vec::with_capacity(1 + 8 + encoded_value.len() + 16);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        buf.extend_from_slice(encoded_value);
        buf.extend_from_slice(&record_id.to_bytes());
        Bytes::from(buf)
    }

    async fn persist_defs(&self) -> DbResult<()> {
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let bytes = bincode::serialize(&defs).map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("sorted-index defs encode: {e}"))
        })?;
        let sys_id = RecordId::system("sorted_indexes");
        self.info_store
            .set(sys_id.to_bytes(), Bytes::from(bytes))
            .await?;
        Ok(())
    }

    async fn load(&self) -> DbResult<()> {
        let sys_id = RecordId::system("sorted_indexes");
        let bytes = match self.info_store.get(sys_id.to_bytes()).await {
            Ok(b) => b,
            Err(_) => return Ok(()),
        };
        if bytes.is_empty() {
            return Ok(());
        }
        // Try the current format first; on failure fall back to the legacy
        // V1 format (no `included_fields`) for backward-compat with existing
        // persisted data written before the covering-index DDL slice.
        let defs: Vec<SortedIndexDefinition> =
            match bincode::deserialize::<Vec<SortedIndexDefinition>>(bytes.as_ref()) {
                Ok(d) => d,
                Err(_) => {
                    let v1s: Vec<SortedIndexDefinitionV1> = bincode::deserialize(bytes.as_ref())
                        .map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "sorted-index defs decode: {e}"
                            ))
                        })?;
                    v1s.into_iter().map(SortedIndexDefinition::from).collect()
                }
            };
        // Last-write-wins dedup by name_interned (matches the previous
        // DashMap::insert loop, on disk Vec is already deduped via
        // persist_defs / iter_indexes but we defensively dedup here too).
        let mut deduped: std::collections::BTreeMap<u64, SortedIndexDefinition> =
            std::collections::BTreeMap::new();
        for d in defs {
            deduped.insert(d.name_interned, d);
        }
        let new_vec: Vec<SortedIndexDefinition> = deduped.into_values().collect();
        self.indexes.store(new_vec);
        Ok(())
    }
}

/// Serialised covering-index projection type: a list of
/// `(field_path_dotted, QueryValue)` pairs, encoded with MessagePack.
///
/// S9: changed from `Vec<(String, InnerValue)>` to `Vec<(String, QueryValue)>`
/// as part of the InnerValue-elimination campaign. `QueryValue = Value<String>`
/// has NO `InternerKey` dependency.
///
/// The field_path_dotted key is the segments joined with "." so the
/// read-side (S3.3) can reconstruct the projection without the interner.
///
/// Format: `rmp_serde::to_vec_named(&Vec<(String, QueryValue)>)` — bincode is
/// not usable because `Value`'s `Deserialize` relies on `deserialize_any`,
/// which bincode does not support.
///
/// The wire format for SCALAR leaves (Null, Bool, Int, F64, Str, Bin, Dec, Big)
/// is byte-identical between `QueryValue` and `InnerValue`, so the decode side
/// (`decode_covering_projection`) can deserialize as `Vec<(String, InnerValue)>`
/// without conversion. Container leaves are skipped at encode time (they would
/// differ due to key types).
type CoveringProjection = Vec<(String, QueryValue)>;

/// Build the covering-index projection value for one record and one
/// `SortedIndexDefinition` that has non-empty `included_fields_interned`.
///
/// S9: produces `Vec<(String, QueryValue)>` instead of `Vec<(String, InnerValue)>`.
/// For each included field path:
///   - Walk `record` by the interned path segments via `RecordRef::materialize_at`.
///   - Convert the leaf to `QueryValue` (scalar-only; containers are skipped).
///   - If the leaf is present, push `(path_joined_with_dots, leaf)` (owned).
///   - Missing / container paths are silently skipped.
///
/// Returns `Bytes::new()` when no fields could be resolved (backward-compat:
/// write side acts as if no projection; read side sees empty value).
///
/// When the projection is non-empty the returned bytes are a **versioned
/// envelope**:
/// ```text
/// [8 bytes: version as u64 little-endian] ++ [msgpack: Vec<(String, QueryValue)>]
/// ```
/// The `version` parameter should be the MVCC write version for the record
/// being indexed (pass `0` when no MVCC store is attached).
fn build_covering_projection(
    record: &(impl RecordRef + ?Sized),
    def: &SortedIndexDefinition,
    version: u64,
) -> Bytes {
    let mut projection: CoveringProjection = Vec::new();
    for (path_strs, path_ids) in def
        .included_fields
        .iter()
        .zip(def.included_fields_interned.iter())
    {
        if path_ids.is_empty() {
            continue;
        }
        let ipath: SmallVec<[InternerKey; 4]> =
            path_ids.iter().map(|&id| InternerKey::new(id)).collect();
        if let Some(leaf) = record.materialize_at(&ipath) {
            if let Some(qv) = inner_value_to_query_scalar(&leaf) {
                let key_str = path_strs.join(".");
                projection.push((key_str, qv));
            }
            // Container leaves (Map/List/Set) are skipped — their QueryValue
            // wire format differs from InnerValue due to key types, and the
            // decode side reads as InnerValue. Scalar leaves are wire-identical.
        }
    }
    if projection.is_empty() {
        return Bytes::new();
    }
    // Use MessagePack (rmp_serde) because Value's Deserialize impl
    // relies on `deserialize_any`, which bincode does not support.
    match rmp_serde::to_vec_named(&projection) {
        Ok(msgpack) => {
            let mut out = version.to_le_bytes().to_vec();
            out.extend_from_slice(&msgpack);
            Bytes::from(out)
        }
        Err(_) => Bytes::new(),
    }
}

/// Convert an `InnerValue` to `QueryValue` for SCALAR types only.
/// Returns `None` for Map/List/Set containers (whose wire format would
/// differ due to InternerKey vs String keys).
fn inner_value_to_query_scalar(v: &InnerValue) -> Option<QueryValue> {
    match v {
        InnerValue::Null => Some(QueryValue::Null),
        InnerValue::Bool(b) => Some(QueryValue::Bool(*b)),
        InnerValue::Int(i) => Some(QueryValue::Int(*i)),
        InnerValue::F64(f) => Some(QueryValue::F64(*f)),
        InnerValue::Dec(d) => Some(QueryValue::Dec(*d)),
        InnerValue::Big(b) => Some(QueryValue::Big(b.clone())),
        InnerValue::Str(s) => Some(QueryValue::Str(s.clone())),
        InnerValue::Bin(b) => Some(QueryValue::Bin(b.clone())),
        // Container leaves are skipped — see doc on build_covering_projection.
        InnerValue::List(_) | InnerValue::Set(_) | InnerValue::Map(_) => None,
    }
}

/// Decode a versioned covering-projection envelope written by
/// `build_covering_projection`. Returns `None` for an empty value, a
/// value shorter than 8 bytes, or one whose msgpack body fails to
/// decode (callers treat `None` as "fall back to a full fetch").
///
/// S9: the encode side writes `Vec<(String, QueryValue)>` but the wire
/// format for scalar leaves is byte-identical to `Vec<(String, InnerValue)>`,
/// so this function can deserialize as `InnerValue` without conversion.
/// The return type stays `InnerValue` for engine API compatibility.
///
/// Used by slice A3 (index-only read path).
pub fn decode_covering_projection(value: &[u8]) -> Option<(u64, Vec<(String, InnerValue)>)> {
    if value.len() < 8 {
        return None;
    }
    let version = u64::from_le_bytes(value[..8].try_into().ok()?);
    let projection: Vec<(String, InnerValue)> = rmp_serde::from_slice(&value[8..]).ok()?;
    Some((version, projection))
}

/// Extract the value at `field_path` from a record and encode it via
/// `sort_codec`. Returns `None` if the field is missing or has a type
/// we don't index (we intentionally skip such records — they won't
/// surface in sorted lookups).
///
/// Reads the scalar via `RecordRef::scalar_at` and dispatches to the
/// SAME `sort_codec::encode_*` primitives the legacy `&InnerValue` path
/// used. `scalar_at` yields exactly {Null, Bool, Int, F64, Str, Bin} for
/// comparable scalars and `None` for Dec/Big/containers/absent —
/// byte-identical to the previous `resolve_path_ref` + InnerValue match,
/// including all the skip cases.
fn extract_and_encode(
    rec: &(impl RecordRef + ?Sized),
    field_path: &[u64],
) -> DbResult<Option<Vec<u8>>> {
    let ipath: SmallVec<[InternerKey; 4]> =
        field_path.iter().map(|&id| InternerKey::new(id)).collect();
    let Some(sr) = rec.scalar_at(&ipath) else {
        return Ok(None);
    };
    let mut buf = Vec::with_capacity(16);
    match sr {
        ScalarRef::Null => sort_codec::encode_null(&mut buf),
        ScalarRef::Bool(b) => sort_codec::encode_bool(&mut buf, b),
        ScalarRef::Int(i) => sort_codec::encode_i64(&mut buf, i),
        ScalarRef::F64(f) => {
            if sort_codec::encode_f64(&mut buf, f).is_err() {
                return Ok(None);
            }
        }
        ScalarRef::Str(s) => sort_codec::encode_str(&mut buf, s),
        ScalarRef::Bin(b) => sort_codec::encode_bytes(&mut buf, b),
    }
    Ok(Some(buf))
}

fn decode_record_id_suffix(key_bytes: &[u8]) -> Option<RecordId> {
    if key_bytes.len() < 16 {
        return None;
    }
    let tail = &key_bytes[key_bytes.len() - 16..];
    let mut arr = [0u8; 16];
    arr.copy_from_slice(tail);
    Some(RecordId(arr))
}
