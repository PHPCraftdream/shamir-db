use shamir_query_types::admin::DropDbOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Drop a database. Optionally attach an HMAC tag.
pub fn drop_db(name: impl Into<String>) -> DropDb {
    DropDb {
        name: name.into(),
        hmac: None,
        cascade: false,
    }
}

/// Builder for [`DropDbOp`] (supports optional HMAC and cascade).
pub struct DropDb {
    name: String,
    hmac: Option<String>,
    cascade: bool,
}

impl DropDb {
    /// Attach the hex-encoded HMAC-SHA256 tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable cascade (drop all child repos and their tables).
    pub fn cascade(mut self) -> Self {
        self.cascade = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropDb(DropDbOp {
            drop_db: self.name,
            hmac: self.hmac,
            cascade: self.cascade,
        })
    }
}

impl From<DropDb> for BatchOp {
    fn from(b: DropDb) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropDb {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
