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

    /// Sign this operation for `db` (the routing database it will execute on)
    /// with the session-derived HMAC key, using the shared canonical v2
    /// encoding. The session key never leaves the caller; nothing is printed.
    /// Only available with the crate's `crypto` feature, which provides the shared canonical encoder.
    #[cfg(feature = "crypto")]
    pub fn sign(self, key: &[u8; 32], db: &str) -> Result<BatchOp, BuilderError> {
        let mut op = ChownOp {
            chown: self.resource,
            owner: self.owner,
            hmac: None,
        };
        let canonical =
            shamir_query_types::hmac::canonical_batch_op(db, &BatchOp::Chown(op.clone()))
                .map_err(|_| BuilderError::HmacSigning)?;
        op.hmac = Some(shamir_query_types::hmac::compute_tag_hex(key, &canonical));
        Ok(BatchOp::Chown(op))
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
