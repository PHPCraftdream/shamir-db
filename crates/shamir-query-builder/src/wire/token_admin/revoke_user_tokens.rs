//! Bulk API-token revocation request builder.
#![warn(missing_docs)]

use shamir_query_types::wire::DbRequest;

/// Start building a request to revoke API tokens for a user.
pub fn revoke_user_tokens() -> RevokeUserTokens {
    RevokeUserTokens {
        user: None,
        hmac: None,
    }
}

/// Builder for a top-level API-token bulk-revocation request.
#[derive(Clone, Debug)]
pub struct RevokeUserTokens {
    user: Option<String>,
    hmac: Option<String>,
}

impl RevokeUserTokens {
    /// Set the raw target username.
    pub fn user(mut self, name: impl Into<String>) -> Self {
        self.user = Some(name.into());
        self
    }

    /// Attach an optional hex-encoded HMAC tag.
    pub fn hmac(mut self, tag: impl Into<String>) -> Self {
        self.hmac = Some(tag.into());
        self
    }

    /// Build the request.
    pub fn build(self) -> DbRequest {
        DbRequest::RevokeUserTokens {
            user: self.user,
            hmac: self.hmac,
        }
    }
}
