//! API-token listing request builder.
#![warn(missing_docs)]

use shamir_query_types::wire::DbRequest;

/// Start building a request to list API tokens.
pub fn list_api_tokens() -> ListApiTokens {
    ListApiTokens {
        for_user: None,
        hmac: None,
    }
}

/// Builder for a top-level API-token listing request.
#[derive(Clone, Debug)]
pub struct ListApiTokens {
    for_user: Option<String>,
    hmac: Option<String>,
}

impl ListApiTokens {
    /// Set the raw target username.
    pub fn for_user(mut self, name: impl Into<String>) -> Self {
        self.for_user = Some(name.into());
        self
    }

    /// Attach an optional hex-encoded HMAC tag.
    pub fn hmac(mut self, tag: impl Into<String>) -> Self {
        self.hmac = Some(tag.into());
        self
    }

    /// Build the request.
    pub fn build(self) -> DbRequest {
        DbRequest::ListApiTokens {
            for_user: self.for_user,
            hmac: self.hmac,
        }
    }
}
