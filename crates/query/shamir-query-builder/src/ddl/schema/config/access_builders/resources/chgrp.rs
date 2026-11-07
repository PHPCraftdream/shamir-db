use crate::batch::TryIntoBatchOp;
use crate::write::BuilderError;
use shamir_query_types::admin::{ChgrpOp, ResourceRef};
use shamir_query_types::batch::BatchOp;

/// Change group on a resource. Pass `None` to clear the group. Returns a
/// builder (HMAC-gated, see [`Chgrp::hmac`]).
pub fn chgrp(resource: ResourceRef, group: Option<u64>) -> ChgrpBuilder {
    ChgrpBuilder {
        resource,
        group,
        hmac: None,
    }
}

/// Builder for [`ChgrpOp`].
pub struct ChgrpBuilder {
    resource: ResourceRef,
    group: Option<u64>,
    hmac: Option<String>,
}

impl ChgrpBuilder {
    /// Attach the hex-encoded HMAC tag.
    /// canonical = `canonical_chgrp(resource, group)`.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`]. Fails with [`BuilderError::MissingHmac`]
    /// when no `.hmac(tag)` was attached.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        let hmac = self.hmac.ok_or(BuilderError::MissingHmac)?;
        Ok(BatchOp::Chgrp(ChgrpOp {
            chgrp: self.resource,
            group: self.group,
            hmac: Some(hmac),
        }))
    }
}

impl TryIntoBatchOp for ChgrpBuilder {
    fn try_into_batch_op(self) -> Result<BatchOp, BuilderError> {
        self.build()
    }
}
