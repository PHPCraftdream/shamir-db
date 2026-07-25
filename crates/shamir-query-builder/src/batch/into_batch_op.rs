use shamir_query_types::batch::BatchOp;
use shamir_query_types::batch::SubBatchOp;
use shamir_query_types::call::CallOp;
use shamir_query_types::read::ReadQuery;
use shamir_query_types::write::{DeleteOp, InsertOp, SetOp, UpdateOp};

/// Anything convertible into a [`BatchOp`].
pub trait IntoBatchOp {
    /// Convert into a batch operation.
    fn into_batch_op(self) -> BatchOp;
}

impl IntoBatchOp for BatchOp {
    fn into_batch_op(self) -> BatchOp {
        self
    }
}

impl IntoBatchOp for ReadQuery {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Read(self)
    }
}

impl IntoBatchOp for crate::query::Query {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Read(self.build())
    }
}

impl IntoBatchOp for InsertOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Insert(self)
    }
}

impl IntoBatchOp for crate::write::Insert {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Insert(self.build())
    }
}

impl IntoBatchOp for UpdateOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Update(self)
    }
}

impl IntoBatchOp for crate::write::Update {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Update(
            self.build()
                .expect("Update::into_batch_op(): set payload is required"),
        )
    }
}

impl IntoBatchOp for SetOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Set(self)
    }
}

impl IntoBatchOp for crate::write::Upsert {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Set(
            self.build()
                .expect("Upsert::into_batch_op(): key and value are required"),
        )
    }
}

impl IntoBatchOp for DeleteOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Delete(self)
    }
}

impl IntoBatchOp for crate::write::Delete {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Delete(
            self.build()
                .expect("Delete::into_batch_op(): where clause is required"),
        )
    }
}

impl IntoBatchOp for CallOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Call(self)
    }
}

impl IntoBatchOp for SubBatchOp {
    fn into_batch_op(self) -> BatchOp {
        BatchOp::Batch(self)
    }
}
