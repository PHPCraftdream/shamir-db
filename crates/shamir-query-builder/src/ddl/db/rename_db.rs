use shamir_query_types::admin::RenameDbOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Rename a database, re-keying every catalogue row that carries its name
/// (databases / repositories / tables) plus the in-memory `DbInstance`.
/// This is a pure catalogue re-key — no files are moved, no handles drained,
/// no stores reopened (campaign ②.1d, variant γ).
///
/// Wire shape: `{ "rename_db": "old", "to": "new" }`.
pub fn rename_db(from: impl Into<String>, to: impl Into<String>) -> RenameDb {
    RenameDb {
        from: from.into(),
        to: to.into(),
    }
}

/// Builder for [`RenameDbOp`].
pub struct RenameDb {
    from: String,
    to: String,
}

impl RenameDb {
    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RenameDb(RenameDbOp {
            rename_db: self.from,
            to: self.to,
        })
    }
}

impl From<RenameDb> for BatchOp {
    fn from(b: RenameDb) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RenameDb {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
