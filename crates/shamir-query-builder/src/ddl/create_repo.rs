use shamir_query_types::admin::CreateRepoOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Create a new repository. Returns a builder for optional fields.
pub fn create_repo(name: impl Into<String>) -> CreateRepo {
    CreateRepo {
        name: name.into(),
        engine: None,
        path: None,
        tables: Vec::new(),
        if_not_exists: false,
    }
}

/// Builder for [`CreateRepoOp`].
pub struct CreateRepo {
    name: String,
    engine: Option<String>,
    path: Option<String>,
    tables: Vec<String>,
    if_not_exists: bool,
}

impl CreateRepo {
    /// Set the storage engine (e.g. `"in_memory"`, `"redb"`, `"fjall"`).
    pub fn engine(mut self, engine: impl Into<String>) -> Self {
        self.engine = Some(engine.into());
        self
    }

    /// Set the data path.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Pre-create these tables inside the repo.
    pub fn tables(mut self, tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tables = tables.into_iter().map(Into::into).collect();
        self
    }

    /// Skip error if the repo already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateRepo(CreateRepoOp {
            create_repo: self.name,
            engine: self.engine,
            path: self.path,
            tables: self.tables,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateRepo> for BatchOp {
    fn from(b: CreateRepo) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
