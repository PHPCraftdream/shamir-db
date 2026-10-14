use shamir_query_types::admin::AlterRepoOp;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::durability::DurabilityOverride;

use crate::batch::IntoBatchOp;

/// Build an ALTER REPO durability settings patch.
pub fn alter_repo(name: impl Into<String>) -> AlterRepo {
    AlterRepo {
        name: name.into(),
        durability: None,
    }
}

/// Builder for an ALTER REPO operation.
pub struct AlterRepo {
    name: String,
    durability: Option<DurabilityOverride>,
}

impl AlterRepo {
    /// Set the sparse durability override patch.
    pub fn durability(mut self, durability: Option<DurabilityOverride>) -> Self {
        self.durability = durability;
        self
    }

    /// Build the query operation.
    pub fn build(self) -> BatchOp {
        BatchOp::AlterRepo(AlterRepoOp {
            alter_repo: self.name,
            durability: self.durability,
        })
    }
}

impl From<AlterRepo> for BatchOp {
    fn from(builder: AlterRepo) -> Self {
        builder.build()
    }
}

impl IntoBatchOp for AlterRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
