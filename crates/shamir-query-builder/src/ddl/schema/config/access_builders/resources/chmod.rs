use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{ChmodOp, ResourceRef};
use shamir_query_types::batch::BatchOp;

/// Change mode bits on a resource. Returns a builder (HMAC-gated, see
/// [`Chmod::hmac`]).
pub fn chmod(resource: ResourceRef, mode: u16) -> Chmod {
    Chmod {
        resource,
        mode,
        hmac: None,
    }
}

/// Builder for [`ChmodOp`].
pub struct Chmod {
    resource: ResourceRef,
    mode: u16,
    hmac: Option<String>,
}

impl Chmod {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_chmod(resource, mode)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::Chmod(ChmodOp {
            chmod: self.resource,
            mode: self.mode,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for Chmod {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
