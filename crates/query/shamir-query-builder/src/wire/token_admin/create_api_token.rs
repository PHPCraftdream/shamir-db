//! API-token creation request builder.
#![warn(missing_docs)]

use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use shamir_query_types::wire::DbRequest;

/// Start building an API-token creation request.
pub fn create_api_token(label: impl Into<String>) -> CreateApiToken {
    CreateApiToken {
        label: label.into(),
        ttl_secs: None,
        mode: None,
        channels: None,
        databases: None,
        for_user: None,
        hmac: None,
    }
}

/// Builder for a top-level API-token creation request.
#[derive(Clone, Debug)]
pub struct CreateApiToken {
    label: String,
    ttl_secs: Option<u64>,
    mode: Option<ApiTokenMode>,
    channels: Option<ApiTokenChannels>,
    databases: Option<Vec<String>>,
    for_user: Option<String>,
    hmac: Option<String>,
}

impl CreateApiToken {
    /// Set token lifetime in seconds.
    pub fn ttl_secs(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = Some(ttl_secs);
        self
    }

    /// Set token access mode.
    pub fn mode(mut self, mode: ApiTokenMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Set permitted request channels.
    pub fn channels(mut self, channels: ApiTokenChannels) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Set the allowed databases. An empty iterator means no databases.
    pub fn databases<I, S>(mut self, databases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.databases = Some(databases.into_iter().map(Into::into).collect());
        self
    }

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

    /// Build the request without client-side validation.
    pub fn build(self) -> DbRequest {
        DbRequest::CreateApiToken {
            label: self.label,
            ttl_secs: self.ttl_secs,
            mode: self.mode,
            channels: self.channels,
            databases: self.databases,
            for_user: self.for_user,
            hmac: self.hmac,
        }
    }
}
