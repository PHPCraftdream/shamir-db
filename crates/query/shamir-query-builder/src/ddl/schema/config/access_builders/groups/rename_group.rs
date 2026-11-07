use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{GroupRef, RenameGroupOp};
use shamir_query_types::batch::BatchOp;

/// Rename a group by reference (name or id) to `to`. Because groups are
/// id-keyed, this only updates the display name; members and resource
/// references (which store the group id) are unaffected. Returns a
/// builder (HMAC-gated, see [`RenameGroup::hmac`]).
pub fn rename_group(group: GroupRef, to: impl Into<String>) -> RenameGroup {
    RenameGroup {
        group,
        to: to.into(),
        hmac: None,
    }
}

/// Builder for [`RenameGroupOp`].
pub struct RenameGroup {
    group: GroupRef,
    to: String,
    hmac: Option<String>,
}

impl RenameGroup {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_rename_group(group, to)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::RenameGroup(RenameGroupOp {
            rename_group: self.group,
            to: self.to,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for RenameGroup {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
