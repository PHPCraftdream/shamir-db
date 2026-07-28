use futures::StreamExt;
use shamir_storage::error::DbResult;

use super::table_manager::TableManager;

impl TableManager {
    /// Register a new sorted (B-tree-by-value) index over a single
    /// scalar field, then backfill it from existing records.
    ///
    /// cancel-safe: NO — `register` persists the definition, then
    /// the backfill streams existing rows into the new index.
    /// Cancellation after register but before/during the backfill
    /// loop leaves a registered sorted index with partial entries;
    /// the doctor's `repair()` rebuilds the index from scratch as a
    /// recovery path. Do NOT call under `tokio::select!` /
    /// `tokio::time::timeout`.
    pub async fn create_sorted_index(&self, index_name: &str, field_path: &[&str]) -> DbResult<()> {
        self.create_sorted_index_with_include(index_name, field_path, Vec::new())
            .await
    }

    /// Create a sorted index, optionally recording covering-index `included_fields`
    /// in the persisted metadata.
    pub async fn create_sorted_index_with_include(
        &self,
        index_name: &str,
        field_path: &[&str],
        included_fields: Vec<Vec<String>>,
    ) -> DbResult<()> {
        use crate::index::sorted_index_manager::SortedIndexDefinition;

        let interner = self.interner.get().await?;
        let name_interned = interner
            .touch_ind(index_name)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
            .key()
            .id();
        let mut path_ids: Vec<u64> = Vec::new();
        for seg in field_path {
            for part in seg.split('.') {
                let id = interner
                    .touch_ind(part)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
                    .key()
                    .id();
                path_ids.push(id);
            }
        }
        // F-42 (#850): the covering-index `included_fields` segments must
        // be interned in-memory BEFORE the durable persist call lands, so
        // their ids are saved in the same chunk as the name + field-path
        // ids. Pre-F-42, `intern_included_paths` ran AFTER register — its
        // `touch_ind` side-effects landed AFTER the persist, so a persist
        // failure (or a crash between persist and `intern_included_paths`)
        // could leave the registered index's covering projection pointing
        // at un-persisted ids. Building `included_fields_interned` inline
        // here and constructing the def with `with_included_interned`
        // pre-populates the transient cache the covering projection reads,
        // so `intern_included_paths` is no longer needed at create time
        // (its other call site in `TableManager::create` still rebuilds
        // the cache for defs loaded from disk after restart).
        let mut included_fields_interned: Vec<Vec<u64>> = Vec::with_capacity(included_fields.len());
        for path_segs in &included_fields {
            let mut seg_ids: Vec<u64> = Vec::with_capacity(path_segs.len());
            for seg in path_segs {
                let id = interner
                    .touch_ind(seg.as_str())
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
                    .key()
                    .id();
                seg_ids.push(id);
            }
            included_fields_interned.push(seg_ids);
        }
        // F-42 (#850) — same fix class as `create_index`/
        // `create_unique_index_locked`: persist the interner's newly-touched
        // ids BEFORE `register` publishes the index. A persist failure
        // aborts before publish, so no rollback is needed (nothing was
        // registered yet). The subsequent `register` + backfill only need
        // the fully-built `def` (whose ids are already in-memory) and the
        // record stream — neither depends on the interner having been
        // durably persisted.
        self.interner.persist().await?;
        let def = SortedIndexDefinition::with_included_interned(
            name_interned,
            path_ids,
            included_fields,
            included_fields_interned,
        );
        self.sorted_indexes.register(def).await?;

        // Backfill: stream existing records and add each to the new
        // sorted index. Avoids materialising the whole table.
        // P4 (pre-refactor boundary): read CURRENT state through the seam
        // (`self.list_stream` → MvccStore::current_stream when attached), not
        // `self.table.list_stream` directly, so collapse-main swaps one place.
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (id, cow) in batch? {
                let record = cow.into_inner()?;
                self.sorted_indexes
                    .on_record_created(&id, &record, 0)
                    .await?;
            }
        }
        Ok(())
    }

    /// Drop a sorted index by name.
    pub async fn drop_sorted_index(&self, index_name: &str) -> DbResult<bool> {
        let interner = self.interner.get().await?;
        let Some(name_interned) = interner.get_ind(index_name) else {
            return Ok(false);
        };
        self.sorted_indexes.drop_index(name_interned.id()).await
    }
}
