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
        let def = SortedIndexDefinition::with_included(name_interned, path_ids, included_fields);
        self.sorted_indexes.register(def).await?;
        // Intern the included_fields paths so the covering projection is
        // active immediately (before backfill).
        self.sorted_indexes.intern_included_paths(interner);
        self.interner.persist().await?;

        // Backfill: stream existing records and add each to the new
        // sorted index. Avoids materialising the whole table.
        // P4 (pre-refactor boundary): read CURRENT state through the seam
        // (`self.list_stream` → MvccStore::current_stream when attached), not
        // `self.table.list_stream` directly, so collapse-main swaps one place.
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (id, record) in batch? {
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
