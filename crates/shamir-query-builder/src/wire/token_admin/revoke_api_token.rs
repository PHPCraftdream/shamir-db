//! API-token revocation request builder.
#![warn(missing_docs)]

use shamir_query_types::wire::DbRequest;

/// Start building a request to revoke one API token.
pub fn revoke_api_token(id: impl Into<String>) -> RevokeApiToken {
    RevokeApiToken {
        id: id.into(),
        hmac: None,
    }
}

/// Builder for a top-level API-token revocation request.
#[derive(Clone, Debug)]
pub struct RevokeApiToken {
    id: String,
    hmac: Option<String>,
}

impl RevokeApiToken {
    /// Attach an optional hex-encoded HMAC tag.
    pub fn hmac(mut self, tag: impl Into<String>) -> Self {
        self.hmac = Some(tag.into());
        self
    }

    /// Build the request.
    pub fn build(self) -> DbRequest {
        DbRequest::RevokeApiToken {
            id: self.id,
            hmac: self.hmac,
        }
    }
}
