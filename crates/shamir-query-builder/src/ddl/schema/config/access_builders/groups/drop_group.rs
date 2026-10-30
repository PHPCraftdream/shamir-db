use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{DropGroupOp, GroupRef};
use shamir_query_types::batch::BatchOp;

/// Drop a group by reference (name or id). Returns a builder for optional
/// flags (HMAC-gated, see [`DropGroup::hmac`]).
pub fn drop_group(group: GroupRef) -> DropGroup {
    DropGroup {
        group,
        if_exists: false,
        hmac: None,
    }
}

/// Builder for [`DropGroupOp`].
pub struct DropGroup {
    group: GroupRef,
    if_exists: bool,
    hmac: Option<String>,
}

impl DropGroup {
    /// Enable `IF EXISTS` semantics: dropping a non-existent group is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_drop_group(group)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::DropGroup(DropGroupOp {
            drop_group: self.group,
            if_exists: self.if_exists,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for DropGroup {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
