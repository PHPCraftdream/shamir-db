use shamir_query_types::admin::{CreateTableOp, Retention};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Create a table. Defaults to `repo = "main"`.
pub fn create_table(name: impl Into<String>) -> CreateTable {
    CreateTable {
        name: name.into(),
        repo: "main".to_owned(),
        if_not_exists: false,
        retention: None,
    }
}

/// Builder for [`CreateTableOp`].
pub struct CreateTable {
    name: String,
    repo: String,
    if_not_exists: bool,
    retention: Option<Retention>,
}

impl CreateTable {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Skip error if the table already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Attach a per-table history-retention policy applied at creation
    /// time. `None` (default) = CurrentOnly.
    pub fn retention(mut self, retention: Retention) -> Self {
        self.retention = Some(retention);
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateTable(CreateTableOp {
            create_table: self.name,
            repo: self.repo,
            if_not_exists: self.if_not_exists,
            retention: self.retention,
        })
    }
}

impl From<CreateTable> for BatchOp {
    fn from(b: CreateTable) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateTable {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
