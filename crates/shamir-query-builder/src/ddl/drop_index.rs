use shamir_query_types::admin::DropIndexOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Drop an index from a table. Returns a builder for optional fields.
pub fn drop_index(name: impl Into<String>, table: impl Into<String>) -> DropIndex {
    DropIndex {
        name: name.into(),
        table: table.into(),
        unique: false,
        repo: "main".to_owned(),
        hmac: None,
    }
}

/// Builder for [`DropIndexOp`].
pub struct DropIndex {
    name: String,
    table: String,
    unique: bool,
    repo: String,
    hmac: Option<String>,
}

impl DropIndex {
    /// Mark that the index being dropped is a unique index.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropIndex(DropIndexOp {
            drop_index: self.name,
            table: self.table,
            unique: self.unique,
            repo: self.repo,
            hmac: self.hmac,
        })
    }
}

impl From<DropIndex> for BatchOp {
    fn from(b: DropIndex) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropIndex {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
