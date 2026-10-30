use crate::batch::IntoBatchOp;
use shamir_query_types::admin::AccessTreeOp;
use shamir_query_types::batch::BatchOp;

/// Request the access-control tree. Returns a builder for optional depth/db.
pub fn access_tree() -> AccessTree {
    AccessTree {
        depth: None,
        db: None,
    }
}

/// Builder for [`AccessTreeOp`].
pub struct AccessTree {
    depth: Option<u32>,
    db: Option<String>,
}

impl AccessTree {
    /// Cap the resource hierarchy depth.
    pub fn depth(mut self, depth: u32) -> Self {
        self.depth = Some(depth);
        self
    }

    /// Restrict the tree to a single database.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = Some(db.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::AccessTree(AccessTreeOp {
            access_tree: true,
            depth: self.depth,
            db: self.db,
        })
    }
}

impl From<AccessTree> for BatchOp {
    fn from(b: AccessTree) -> Self {
        b.build()
    }
}

impl IntoBatchOp for AccessTree {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
