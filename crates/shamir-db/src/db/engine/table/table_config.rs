#[derive(Debug, Clone)]
pub struct TableConfig {
    pub name: String,
    pub enable_indexes: bool,
}

impl TableConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enable_indexes: false,
        }
    }

    pub fn with_indexes(mut self) -> Self {
        self.enable_indexes = true;
        self
    }
}
