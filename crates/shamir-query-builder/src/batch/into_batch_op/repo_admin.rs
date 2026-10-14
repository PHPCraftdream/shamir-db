use super::Batch;
use crate::batch::{Handle, IntoBatchOp};
use shamir_query_types::batch::BatchOp;

impl Batch {
    pub(super) fn add_entry(
        &mut self,
        alias: impl Into<String>,
        op: BatchOp,
        return_result: bool,
    ) -> Handle {
        self.add_entry_after(alias, op, return_result, &[])
    }

    /// Add an ALTER REPO settings patch.
    pub fn alter_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add a DESCRIBE REPO request.
    pub fn describe_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }
}
