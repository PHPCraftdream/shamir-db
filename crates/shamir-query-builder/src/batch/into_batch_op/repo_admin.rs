use super::batch::Batch;
use super::{Handle, IntoBatchOp};

impl Batch {
    /// Add an ALTER REPO settings patch.
    pub fn alter_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }

    /// Add a DESCRIBE REPO request.
    pub fn describe_repo(&mut self, alias: impl Into<String>, op: impl IntoBatchOp) -> Handle {
        self.add_entry(alias, op.into_batch_op(), true)
    }
}
