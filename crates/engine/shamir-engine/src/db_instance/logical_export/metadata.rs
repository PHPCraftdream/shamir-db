use super::MaintenanceGuard;
use crate::table::TableManager;
use shamir_index::base_index::index_definition::IndexDefinition;
use shamir_index::base_index::sorted_index_definition::SortedIndexDefinition;
use shamir_index::descriptor::IndexDescriptor;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_membuffer::MemBufferConfig;
use shamir_types::core::interner::InternerKey;
use std::sync::Arc;

/// Borrowed read-only metadata adapter. Raw engine index descriptors are NOT
/// wire payloads: resolve their name/path ids via `name` before typed encoding.
/// Functional expression field paths must be resolved recursively as well.
/// No derived postings, native caches or physical store ids are returned.
pub struct LogicalTable<'a> {
    table: TableManager,
    guard: &'a MaintenanceGuard,
}

impl<'a> LogicalTable<'a> {
    pub(super) fn new(table: TableManager, guard: &'a MaintenanceGuard) -> Self {
        Self { table, guard }
    }

    pub async fn buffer_config(&self) -> DbResult<Option<MemBufferConfig>> {
        self.guard.check_deadline()?;
        self.table.get_buffer_config().await
    }

    pub async fn name(&self, id: u64) -> DbResult<String> {
        self.guard.check_deadline()?;
        self.table
            .info_store_read_view()
            .interner_read_view()
            .await?
            .get_str(&InternerKey::new(id))
            .map(|name| name.to_string())
            .ok_or_else(|| DbError::Codec(format!("unresolved descriptor name {id}")))
    }

    /// Existing iterator, consumed one descriptor at a time without collecting
    /// the table's descriptor set. The caller enforces aggregate metadata budget.
    pub fn regular_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.table.index_manager_ref().iter_indexes()
    }

    pub fn unique_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.table.index_manager_ref().iter_unique_indexes()
    }

    /// Shared resident snapshot, not a cloned catalogue. Consumers page/slice
    /// it and must resolve ids, omit provenance and check container budgets.
    pub fn sorted_indexes(&self) -> Arc<Vec<SortedIndexDefinition>> {
        self.table.sorted_indexes().snapshot()
    }

    /// Raw descriptors are read one id at a time; gaps (dropped indexes) are
    /// skipped. The returned resume id is exclusive and zero starts the walk.
    pub async fn next_index2(&self, after_id: u32) -> DbResult<Option<(u32, IndexDescriptor)>> {
        self.guard.check_deadline()?;
        let registry = self.table.index2_registry();
        let ceiling = registry.peek_next_id();
        let Some(start) = after_id.checked_add(1) else {
            return Ok(None);
        };
        for id in start..ceiling {
            if let Some(backend) = registry.get_by_id(id).await {
                let mut descriptor = backend.persisted_descriptor();
                descriptor.state = registry
                    .state_of(id)
                    .await
                    .ok_or_else(|| DbError::Conflict("index disappeared during export".into()))?;
                // Existing all_descriptors is the authoritative rename adapter;
                // backend descriptors can retain old names. Fail closed rather
                // than exporting an unverified renamed identity.
                let name = self.name(descriptor.name_interned).await?;
                let current = registry.get_by_name(descriptor.name_interned).await;
                if name != descriptor.name
                    || !current
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &backend))
                {
                    return Err(DbError::Conflict("index descriptor name mismatch".into()));
                }
                return Ok(Some((id, descriptor)));
            }
            if id % 256 == 0 {
                self.guard.check_deadline()?;
                tokio::task::yield_now().await;
            }
        }
        Ok(None)
    }

    pub fn bindings(&self) -> Arc<Vec<shamir_engine_validator::validator::ValidatorBinding>> {
        self.table.validator_bindings()
    }

    pub fn transforms(
        &self,
    ) -> Vec<(
        Vec<String>,
        shamir_engine_validator::validator::TransformSpec,
    )> {
        self.table.schema_transforms()
    }
}
