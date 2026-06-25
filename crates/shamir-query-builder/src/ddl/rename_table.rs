use shamir_query_types::admin::RenameTableOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Rename a table inside a repository. Defaults to `repo = "main"`.
///
/// Wire shape: `{ "rename_table": "old", "to": "new", "repo": "main" }`.
pub fn rename_table(from: impl Into<String>, to: impl Into<String>) -> RenameTable {
    RenameTable {
        from: from.into(),
        to: to.into(),
        repo: "main".to_owned(),
    }
}

/// Builder for [`RenameTableOp`].
pub struct RenameTable {
    from: String,
    to: String,
    repo: String,
}

impl RenameTable {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RenameTable(RenameTableOp {
            rename_table: self.from,
            to: self.to,
            repo: self.repo,
        })
    }
}

impl From<RenameTable> for BatchOp {
    fn from(b: RenameTable) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RenameTable {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
