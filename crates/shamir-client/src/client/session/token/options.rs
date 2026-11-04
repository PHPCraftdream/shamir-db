use std::time::Duration;

use super::api_token::ApiToken;
use crate::client::transport::token::TokenEndpoint;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Options for [`TokenHttpClient::connect`](super::TokenHttpClient::connect).
#[non_exhaustive]
#[derive(Debug)]
pub struct TokenHttpOptions {
    pub endpoint: TokenEndpoint,
    pub token: ApiToken,
    pub connect_timeout: Option<Duration>,
    pub request_timeout: Option<Duration>,
}

impl TokenHttpOptions {
    pub fn new(endpoint: TokenEndpoint, token: ApiToken) -> Self {
        Self {
            endpoint,
            token,
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        }
    }

    pub fn with_connect_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_request_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.request_timeout = timeout;
        self
    }
}
