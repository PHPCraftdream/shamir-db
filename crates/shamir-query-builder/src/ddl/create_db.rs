use shamir_query_types::admin::CreateDbOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Create a new database. Returns a builder for optional `if_not_exists`.
pub fn create_db(name: impl Into<String>) -> CreateDb {
    CreateDb {
        name: name.into(),
        if_not_exists: false,
    }
}

/// Builder for [`CreateDbOp`].
pub struct CreateDb {
    name: String,
    if_not_exists: bool,
}

impl CreateDb {
    /// Skip error if the database already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateDb(CreateDbOp {
            create_db: self.name,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateDb> for BatchOp {
    fn from(b: CreateDb) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateDb {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
