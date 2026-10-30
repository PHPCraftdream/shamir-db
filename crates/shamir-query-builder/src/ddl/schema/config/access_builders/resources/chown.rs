use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{ChownOp, ResourceRef};
use shamir_query_types::batch::BatchOp;

/// Change owner on a resource. Returns a builder (HMAC-gated, see
/// [`Chown::hmac`]).
pub fn chown(resource: ResourceRef, owner: u64) -> Chown {
    Chown {
        resource,
        owner,
        hmac: None,
    }
}

/// Builder for [`ChownOp`].
pub struct Chown {
    resource: ResourceRef,
    owner: u64,
    hmac: Option<String>,
}

impl Chown {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_chown(resource, owner)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::Chown(ChownOp {
            chown: self.resource,
            owner: self.owner,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for Chown {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
