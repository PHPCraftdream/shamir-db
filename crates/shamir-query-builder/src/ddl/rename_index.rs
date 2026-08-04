use shamir_query_types::admin::RenameIndexOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Rename an index on a table (in-place rekey, no data loss).
///
/// Defaults to `repo = "main"`.
///
/// Wire shape:
/// `{ "rename_index": "old", "to": "new", "table": "users", "repo": "main" }`.
pub fn rename_index(
    table: impl Into<String>,
    from: impl Into<String>,
    to: impl Into<String>,
) -> RenameIndex {
    RenameIndex {
        table: table.into(),
        from: from.into(),
        to: to.into(),
        repo: "main".to_owned(),
        if_exists: false,
    }
}

/// Builder for [`RenameIndexOp`].
pub struct RenameIndex {
    table: String,
    from: String,
    to: String,
    repo: String,
    if_exists: bool,
}

impl RenameIndex {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Skip error if the source index does not exist (mirrors
    /// `CreateIndex::if_not_exists` / `DropIndex::if_exists`).
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RenameIndex(RenameIndexOp {
            rename_index: self.from,
            to: self.to,
            table: self.table,
            repo: self.repo,
            if_exists: self.if_exists,
        })
    }
}

impl From<RenameIndex> for BatchOp {
    fn from(b: RenameIndex) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RenameIndex {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
