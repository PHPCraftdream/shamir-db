use std::fmt;
use std::time::Duration;

use zeroize::Zeroizing;

use super::login_scope::LoginScope;
use crate::client::transport::token::TokenEndpoint;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Inputs of a password login over the server HTTP listener.
#[non_exhaustive]
pub struct LoginOptions {
    pub endpoint: TokenEndpoint,
    pub username: String,
    pub password: Zeroizing<String>,
    pub scope: LoginScope,
    /// Pin of the expected listener id; `None` accepts whatever the server reports.
    pub listener_id: Option<[u8; 16]>,
    pub connect_timeout: Option<Duration>,
    pub request_timeout: Option<Duration>,
}

impl LoginOptions {
    pub fn new(
        endpoint: TokenEndpoint,
        username: impl Into<String>,
        password: Zeroizing<String>,
    ) -> Self {
        Self {
            endpoint,
            username: username.into(),
            password,
            scope: LoginScope::default(),
            listener_id: None,
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        }
    }

    pub fn with_scope(mut self, scope: LoginScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_listener_id(mut self, listener_id: Option<[u8; 16]>) -> Self {
        self.listener_id = listener_id;
        self
    }

    pub fn with_connect_timeout(mut self, connect_timeout: Option<Duration>) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    pub fn with_request_timeout(mut self, request_timeout: Option<Duration>) -> Self {
        self.request_timeout = request_timeout;
        self
    }
}

impl fmt::Debug for LoginOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginOptions")
            .field("endpoint", &self.endpoint)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("scope", &self.scope)
            .field("listener_id", &self.listener_id)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}
