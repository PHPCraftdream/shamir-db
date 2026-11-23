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

    /// Sign this operation for `db` (the routing database it will execute on)
    /// with the session-derived HMAC key, using the shared canonical v2
    /// encoding. The session key never leaves the caller; nothing is printed.
    /// Only available with the crate's `crypto` feature, which provides the shared canonical encoder.
    #[cfg(feature = "crypto")]
    pub fn sign(self, key: &[u8; 32], db: &str) -> Result<BatchOp, BuilderError> {
        let mut op = ChgrpOp {
            chgrp: self.resource,
            group: self.group,
            hmac: None,
        };
        let canonical =
            shamir_query_types::hmac::canonical_batch_op(db, &BatchOp::Chgrp(op.clone()))
                .map_err(|_| BuilderError::HmacSigning)?;
        op.hmac = Some(shamir_query_types::hmac::compute_tag_hex(key, &canonical));
        Ok(BatchOp::Chgrp(op))
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
