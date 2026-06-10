use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::TouchInd;

use super::table_manager::TableManager;

impl TableManager {
    /// Replicate src's interner state into this TableManager's info_store.
    ///
    /// Migration copies raw `data_store` bytes, which embed `InternerKey(u64)`
    /// references for field names. For those bytes to decode correctly on
    /// dst, dst's interner must hold the **same** `id → name` mappings as
    /// src. The interner persists itself under a fixed system key, so we
    /// just copy that one record byte-for-byte and let the dst interner
    /// pick it up via its normal lazy-load path.
    ///
    /// Must be called BEFORE any `.interner().get()` on `self` (so the
    /// lazy load sees the freshly-copied bytes) and BEFORE
    /// `replicate_index2_descriptors_from` (which re-interns names on dst).
    /// Persists src first so the bytes are current.
    pub async fn replicate_interner_from(&self, src: &TableManager) -> DbResult<()> {
        src.interner().persist().await?;
        // Copy the legacy single-blob record (if any) — for repos
        // upgraded from the old persist format. New code never writes
        // here, but on-disk data may still contain it.
        let legacy_key = crate::meta::MetaKey::Internals.as_record_id().to_bytes();
        match src.info_store.get(legacy_key.clone()).await {
            Ok(bytes) => {
                self.info_store.set(legacy_key, bytes).await?;
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => {
                // No legacy blob — fall through to chunk copy.
            }
            Err(e) => return Err(e),
        }
        // Copy every append-only delta chunk emitted by the new
        // incremental persist path. Without this, the dst would load
        // an empty interner and msgpack-encoded field-name ids on dst
        // would not resolve.
        let prefix = {
            let mut p = Vec::with_capacity(4 + 3);
            p.extend_from_slice(&[0u8, 0, 0, 0]);
            p.extend_from_slice(b"i.d");
            bytes::Bytes::from(p)
        };
        let mut stream = src.info_store.scan_prefix_stream(prefix, 256);
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                self.info_store.set(k, v).await?;
            }
        }
        Ok(())
    }

    /// Replicate src's index2 descriptors onto this TableManager.
    ///
    /// For each non-Btree descriptor on `src`:
    /// 1. Intern the name + path segments in **this** manager's interner
    ///    (so `name_interned` and `paths` resolve correctly in the dst
    ///    address space).
    /// 2. Allocate a fresh local `id` (src ids are a separate counter).
    /// 3. Build a backend via `build_index2_backend` (empty — no postings
    ///    yet) and register it in this registry.
    /// 4. Persist metadata.
    ///
    /// Must be called **before** `bulk_populate_index2` and **before**
    /// any writes reach the dst table.
    pub async fn replicate_index2_descriptors_from(&self, src: &TableManager) -> DbResult<()> {
        let src_backends = src.index2_registry.all_backends().await;
        if src_backends.is_empty() {
            return Ok(());
        }

        let interner = self.interner.get().await?;

        for src_backend in &src_backends {
            let src_desc = src_backend.descriptor();

            // Intern name in dst address space.
            let name_key = match interner
                .touch_ind(&src_desc.name)
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
            {
                TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
            };

            // Intern each path segment in dst address space.
            let mut interned_paths: smallvec::SmallVec<[Vec<u64>; 2]> = smallvec::SmallVec::new();
            for path in &src_desc.paths {
                let mut seg_ids = Vec::with_capacity(path.len());
                // The src's `paths` already contain interned u64s from
                // src's interner — but we need the original field names
                // to re-intern on dst. Recover them via src's interner.
                let src_interner = src.interner.get().await?;
                for &seg_u64 in path {
                    let seg_str = src_interner
                        .get_str(&src_interner.make_key(seg_u64))
                        .ok_or_else(|| {
                            shamir_storage::error::DbError::Internal(format!(
                                "cannot resolve interned segment {} from src",
                                seg_u64
                            ))
                        })?;
                    let dst_key = match interner
                        .touch_ind(seg_str)
                        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
                    {
                        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
                    };
                    seg_ids.push(dst_key);
                }
                interned_paths.push(seg_ids);
            }

            let new_id = self.index2_registry.allocate_id();
            let new_desc = crate::index2::descriptor::IndexDescriptor::new(
                new_id,
                src_desc.name.clone(),
                name_key,
                interned_paths,
                src_desc.kind.clone(),
            );

            let backend = crate::index2::build_index2_backend(new_desc, &self.info_store);
            self.index2_registry
                .insert(backend)
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
        }

        self.interner.persist().await?;
        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await?;

        Ok(())
    }

    /// C2 (collapse-main bridge): seed the version log from this table's raw
    /// `data_store`. A migration cutover copies records straight into
    /// `data_store` (the coordinator's `final_drain_and_commit` writes
    /// data_store only, bypassing the version log). Reads now resolve from the
    /// log, so this pushes every record through `set_versioned_many` to make it
    /// a current version in the log (+ cell). No-op when no `MvccStore` is
    /// attached (system/test tables). The migration coordinator will write the
    /// log directly in a later slice (C5); this bridges it at cutover. Call it
    /// after `final_drain_and_commit` and before `bulk_populate_index2` (which
    /// streams via the log-backed seam).
    pub async fn seed_log_from_data_store(&self) -> DbResult<()> {
        let Some(mvcc) = self.mvcc_store_ref() else {
            return Ok(());
        };
        let stream = self.table.data_store().iter_stream(256);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            let items = batch?;
            if !items.is_empty() {
                mvcc.set_versioned_many(items).await?;
            }
        }
        Ok(())
    }

    /// Bulk-populate all index2 backends by streaming records from this
    /// TableManager's data_store and calling `plan_insert + apply_index_ops`
    /// for each record on every registered backend.
    ///
    /// This creates postings in info_store **and** populates in-memory
    /// state (HNSW graph, BM25 counters, etc.). Intended for migration
    /// cutover — the dst table has data_store populated by snapshot/drain
    /// but its info_store is empty.
    ///
    /// Must be called **after** `replicate_index2_descriptors_from` and
    /// **after** all data has landed in the dst data_store (i.e. after
    /// `drain_until_caught_up`). New writes after `bulk_populate_index2`
    /// must go through `insert()` which calls `index2_on_insert` — the
    /// migration coordinator's `final_drain_and_commit` writes directly
    /// to `dst_data` (data_store only) and does **not** trigger index2
    /// hooks. Therefore `bulk_populate_index2` should be called **after**
    /// `final_drain_and_commit` if any shadow-log entries may have been
    /// written between `drain_until_caught_up` and `mark_cutover_ready`.
    pub async fn bulk_populate_index2(&self) -> DbResult<()> {
        let backends = self.index2_registry.all_backends().await;
        if backends.is_empty() {
            return Ok(());
        }

        // P4 (pre-refactor boundary): read CURRENT state through the seam
        // (`self.list_stream` → MvccStore::current_stream when attached) so
        // collapse-main reroutes index2 backfill in one place.
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let items: Vec<(
                shamir_types::types::record_id::RecordId,
                &shamir_types::types::value::InnerValue,
            )> = batch.iter().map(|(rid, val)| (*rid, val)).collect();
            for backend in &backends {
                for (rid, val) in items.iter() {
                    let ops = backend.plan_insert(*rid, val).await.map_err(|e| {
                        shamir_storage::error::DbError::Internal(format!(
                            "bulk_populate_index2 plan_insert failed: {}",
                            e
                        ))
                    })?;
                    crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref())
                        .await
                        .map_err(|e| {
                            shamir_storage::error::DbError::Internal(format!(
                                "bulk_populate_index2 apply_index_ops failed: {}",
                                e
                            ))
                        })?;
                }
            }
        }

        Ok(())
    }
}
