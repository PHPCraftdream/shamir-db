use shamir_query_types::admin::DropRepoOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Drop a repository. Returns a builder for optional HMAC.
pub fn drop_repo(name: impl Into<String>) -> DropRepo {
    DropRepo {
        name: name.into(),
        hmac: None,
        cascade: false,
    }
}

/// Builder for [`DropRepoOp`].
pub struct DropRepo {
    name: String,
    hmac: Option<String>,
    cascade: bool,
}

impl DropRepo {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable cascade (drop all child tables).
    pub fn cascade(mut self) -> Self {
        self.cascade = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropRepo(DropRepoOp {
            drop_repo: self.name,
            hmac: self.hmac,
            cascade: self.cascade,
        })
    }
}

impl From<DropRepo> for BatchOp {
    fn from(b: DropRepo) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
