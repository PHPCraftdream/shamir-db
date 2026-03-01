use super::repo_types::BoxRepoFactory;
use crate::db::engine::table::TableConfig;

#[derive(Clone)]
pub struct RepoConfig {
    pub name: String,
    pub factory: BoxRepoFactory,
    pub tables: Vec<TableConfig>,
}

impl RepoConfig {
    pub fn new(name: impl Into<String>, factory: BoxRepoFactory) -> Self {
        Self {
            name: name.into(),
            factory,
            tables: Vec::new(),
        }
    }

    pub fn add_table(mut self, table_config: TableConfig) -> Self {
        self.tables.push(table_config);
        self
    }

    pub fn add_tables(mut self, table_configs: Vec<TableConfig>) -> Self {
        self.tables.extend(table_configs);
        self
    }
}
