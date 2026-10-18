use shamir_query_types::admin::RenameRepoOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Rename a repository inside the current database, preserving all of its
/// tables, data, indexes, and catalogue metadata.
///
/// Wire shape: `{ "rename_repo": "old", "to": "new" }`.
pub fn rename_repo(from: impl Into<String>, to: impl Into<String>) -> RenameRepo {
    RenameRepo {
        from: from.into(),
        to: to.into(),
    }
}

/// Builder for [`RenameRepoOp`].
pub struct RenameRepo {
    from: String,
    to: String,
}

impl RenameRepo {
    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RenameRepo(RenameRepoOp {
            rename_repo: self.from,
            to: self.to,
        })
    }
}

impl From<RenameRepo> for BatchOp {
    fn from(b: RenameRepo) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RenameRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
