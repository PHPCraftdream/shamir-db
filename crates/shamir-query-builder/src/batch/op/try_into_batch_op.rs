use shamir_query_types::batch::BatchOp;

use crate::write::BuilderError;

/// Fallible counterpart of [`IntoBatchOp`]: convert `self` into a [`BatchOp`],
/// returning a typed [`BuilderError`] when a required builder field was never
/// set, instead of panicking.
///
/// Implemented for exactly the five write/DDL builder types whose `build()`
/// methods return `Result<_, BuilderError>`:
/// - [`crate::write::Update`]
/// - [`crate::write::Upsert`]
/// - [`crate::write::Delete`]
/// - [`crate::ddl::AddSchemaRuleBuilder`]
/// - [`crate::ddl::AlterSubscriptionBuilder`]
///
/// Every other `IntoBatchOp` impl wraps an already-fully-specified DTO whose
/// `build()` is infallible, so it has no fallible counterpart here.
///
/// Use this via [`crate::batch::Batch::try_op`] (the fallible mirror of
/// [`crate::batch::Batch::op`]) or the named `try_*` convenience methods
/// ([`crate::batch::Batch::try_update`], …) to surface a malformed op as a
/// typed error rather than a panic.
pub trait TryIntoBatchOp {
    /// Convert into a [`BatchOp`], or return the typed [`BuilderError`].
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError>;
}

impl TryIntoBatchOp for crate::write::Update {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        Ok(BatchOp::Update(self.build()?))
    }
}

impl TryIntoBatchOp for crate::write::Upsert {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        Ok(BatchOp::Set(self.build()?))
    }
}

impl TryIntoBatchOp for crate::write::Delete {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        Ok(BatchOp::Delete(self.build()?))
    }
}

impl TryIntoBatchOp for crate::ddl::AddSchemaRuleBuilder {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}

impl TryIntoBatchOp for crate::ddl::AlterSubscriptionBuilder {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
