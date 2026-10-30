use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{GroupRef, RemoveGroupMemberOp};
use shamir_query_types::batch::BatchOp;

/// Remove a user from a group. Returns a builder (HMAC-gated, see
/// [`RemoveGroupMember::hmac`]).
pub fn remove_group_member(group: GroupRef, user: u64) -> RemoveGroupMember {
    RemoveGroupMember {
        group,
        user,
        hmac: None,
    }
}

/// Builder for [`RemoveGroupMemberOp`].
pub struct RemoveGroupMember {
    group: GroupRef,
    user: u64,
    hmac: Option<String>,
}

impl RemoveGroupMember {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_remove_group_member(group, user)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::RemoveGroupMember(RemoveGroupMemberOp {
            remove_group_member: self.group,
            user: self.user,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for RemoveGroupMember {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
