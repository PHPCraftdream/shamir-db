use crate::repo::instance::repo_instance::table_store::new_table_store_id;
use shamir_types::types::record_id::RecordId;

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
}
