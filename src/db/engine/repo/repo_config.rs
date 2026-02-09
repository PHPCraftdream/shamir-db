use super::repo_types::BoxRepo;
use crate::db::engine::table::TableConfig;

#[derive(Clone)]
pub struct RepoConfig {
    pub name: String,
    pub repo: BoxRepo,
    pub tables: Vec<TableConfig>,
}

impl RepoConfig {
    pub fn new(name: impl Into<String>, repo: BoxRepo) -> Self {
        Self {
            name: name.into(),
            repo,
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
