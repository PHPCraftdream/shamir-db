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

    /// Sign this operation for `db` (the routing database it will execute on)
    /// with the session-derived HMAC key, using the shared canonical v2
    /// encoding. The session key never leaves the caller; nothing is printed.
    pub fn sign(self, key: &[u8; 32], db: &str) -> Result<BatchOp, BuilderError> {
        let mut op = ChmodOp {
            chmod: self.resource,
            mode: self.mode,
            hmac: None,
        };
        let canonical =
            shamir_query_types::hmac::canonical_batch_op(db, &BatchOp::Chmod(op.clone()))
                .map_err(|_| BuilderError::HmacSigning)?;
        op.hmac = Some(shamir_query_types::hmac::compute_tag_hex(key, &canonical));
        Ok(BatchOp::Chmod(op))
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
