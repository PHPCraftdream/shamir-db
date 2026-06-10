use shamir_query_types::admin::DropTableOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Drop a table. Defaults to `repo = "main"`.
pub fn drop_table(name: impl Into<String>) -> DropTable {
    DropTable {
        name: name.into(),
        repo: "main".to_owned(),
        hmac: None,
    }
}

/// Builder for [`DropTableOp`].
pub struct DropTable {
    name: String,
    repo: String,
    hmac: Option<String>,
}

impl DropTable {
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
        BatchOp::DropTable(DropTableOp {
            drop_table: self.name,
            repo: self.repo,
            hmac: self.hmac,
        })
    }
}

impl From<DropTable> for BatchOp {
    fn from(b: DropTable) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropTable {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
