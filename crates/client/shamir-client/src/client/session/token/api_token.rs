use std::fmt;

use bytes::Bytes;
use hyper::http::HeaderValue;
use shamir_connect::server::token::parse_token;
use shamir_query_types::wire::token_admin::CreatedApiToken;
use zeroize::{Zeroize, Zeroizing};

use crate::error::ClientError;

const BEARER_PREFIX: &[u8] = b"Bearer ";

struct ZeroizingBytes(Vec<u8>);

impl AsRef<[u8]> for ZeroizingBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for ZeroizingBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A validated `shk1.` API token. The secret is zeroized on drop and never
/// appears in `Debug` output.
#[non_exhaustive]
pub struct ApiToken(Zeroizing<String>);

impl ApiToken {
    /// Validates the canonical token format; the input is never echoed in the
    /// error and is wiped on failure.
    pub fn new(token: impl Into<String>) -> Result<Self, ClientError> {
        let token = Zeroizing::new(token.into());
        parse_token(token.as_bytes()).map_err(|_| ClientError::InvalidToken)?;
        Ok(Self(token))
    }

    /// Non-secret token identifier (base64url).
    pub fn id(&self) -> String {
        match parse_token(self.0.as_bytes()) {
            Ok(parsed) => parsed.id.to_b64(),
            Err(_) => String::new(),
        }
    }

    /// The raw secret token. Handle with care; never log it.
    pub fn reveal(&self) -> &str {
        &self.0
    }

    pub(crate) fn authorization_header(&self) -> Result<HeaderValue, ClientError> {
        let token = self.0.as_bytes();
        let mut buf = Vec::with_capacity(BEARER_PREFIX.len() + token.len());
        buf.extend_from_slice(BEARER_PREFIX);
        buf.extend_from_slice(token);
        let owner = ZeroizingBytes(buf);
        let mut value = HeaderValue::from_maybe_shared(Bytes::from_owner(owner))
            .map_err(|_| ClientError::InvalidToken)?;
        value.set_sensitive(true);
        Ok(value)
    }
}

impl fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiToken")
            .field("id", &self.id())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl TryFrom<&str> for ApiToken {
    type Error = ClientError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&CreatedApiToken> for ApiToken {
    type Error = ClientError;

    fn try_from(value: &CreatedApiToken) -> Result<Self, Self::Error> {
        Self::new(value.token.reveal())
    }
}
