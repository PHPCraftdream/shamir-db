use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{AddGroupMemberOp, GroupRef};
use shamir_query_types::batch::BatchOp;

/// Add a user to a group. Returns a builder (HMAC-gated, see
/// [`AddGroupMember::hmac`]).
pub fn add_group_member(group: GroupRef, user: u64) -> AddGroupMember {
    AddGroupMember {
        group,
        user,
        hmac: None,
    }
}

/// Builder for [`AddGroupMemberOp`].
pub struct AddGroupMember {
    group: GroupRef,
    user: u64,
    hmac: Option<String>,
}

impl AddGroupMember {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_add_group_member(group, user)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::AddGroupMember(AddGroupMemberOp {
            add_group_member: self.group,
            user: self.user,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for AddGroupMember {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
