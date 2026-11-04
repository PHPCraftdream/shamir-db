use std::time::Duration;

use super::api_token::ApiToken;
use crate::client::transport::token::TokenEndpoint;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
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

/// Options for [`Client::connect_token_ws`](crate::Client::connect_token_ws).
///
/// `endpoint` must point at a `kind: http` listener; the route is
/// `GET /shamir/v1/ws`.
#[non_exhaustive]
#[derive(Debug)]
pub struct TokenWsOptions {
    pub endpoint: TokenEndpoint,
    pub token: ApiToken,
    pub connect_timeout: Option<Duration>,
    pub handshake_timeout: Option<Duration>,
    pub request_timeout: Option<Duration>,
}

impl TokenWsOptions {
    pub fn new(endpoint: TokenEndpoint, token: ApiToken) -> Self {
        Self {
            endpoint,
            token,
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            handshake_timeout: Some(DEFAULT_HANDSHAKE_TIMEOUT),
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        }
    }

    pub fn with_connect_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_handshake_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    pub fn with_request_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.request_timeout = timeout;
        self
    }
}
