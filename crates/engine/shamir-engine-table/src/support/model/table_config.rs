use super::names::{new_table_store_id, validate_table_store_name};
use crate::TableManager;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;
use tokio::sync::OnceCell;

#[derive(Debug, Clone)]
pub struct TableConfig {
    pub name: String,
    pub enable_indexes: bool,
    pub store_id: Option<RecordId>,
}

impl TableConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enable_indexes: false,
            store_id: None,
        }
    }

    pub fn with_indexes(mut self) -> Self {
        self.enable_indexes = true;
        self
    }

    pub fn with_new_store_id(self) -> Self {
        self.with_store_id(new_table_store_id())
    }

    pub fn with_store_id(mut self, store_id: RecordId) -> Self {
        self.store_id = Some(store_id);
        self
    }

    /// Validate the mapped names for a persisted incarnation. Id-less legacy
    /// mappings remain unchanged and do not receive the generated suffix.
    pub fn validate_store_name(&self) -> DbResult<()> {
        let Some(store_id) = self.store_id.as_ref() else {
            return Ok(());
        };
        if validate_table_store_name(&self.name, store_id) {
            Ok(())
        } else {
            Err(DbError::Validation(format!(
                "table name '{}' is too long for generated store names",
                self.name
            )))
        }
    }
}

/// Identity of ONE live table registration: the catalogue cell the registry
/// handed out for a configured table name. Two DIFFERENT registrations under
/// the same name (drop + re-create, or another repository taking the same
/// route) always hold distinct cells, and holding this token keeps the cell
/// alive, so no second registration can be allocated at the same address
/// while the token lives. `TableConfig::store_id` alone cannot serve this
/// purpose: a legacy table loaded from the catalogue has `store_id: None`
/// (`table_config_from_catalogue_record`), so two distinct legacy
/// registrations would compare equal.
pub struct TableRegistration(Arc<OnceCell<TableManager>>);

impl TableRegistration {
    pub(crate) fn of(cell: Arc<OnceCell<TableManager>>) -> Self {
        Self(cell)
    }

    /// Whether both tokens name the SAME live registration.
    pub fn same_registration(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
