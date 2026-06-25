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
    }
}

/// Builder for [`RenameIndexOp`].
pub struct RenameIndex {
    table: String,
    from: String,
    to: String,
    repo: String,
}

impl RenameIndex {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RenameIndex(RenameIndexOp {
            rename_index: self.from,
            to: self.to,
            table: self.table,
            repo: self.repo,
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
