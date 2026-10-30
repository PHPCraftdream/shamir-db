use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::CreateGroupOp;
use shamir_query_types::batch::BatchOp;

/// Create a new group. Returns a builder (HMAC-gated, see
/// [`CreateGroup::hmac`]).
pub fn create_group(name: impl Into<String>) -> CreateGroup {
    CreateGroup {
        name: name.into(),
        hmac: None,
    }
}

/// Builder for [`CreateGroupOp`].
pub struct CreateGroup {
    name: String,
    hmac: Option<String>,
}

impl CreateGroup {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_create_group(name)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::CreateGroup(CreateGroupOp {
            create_group: self.name,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for CreateGroup {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
