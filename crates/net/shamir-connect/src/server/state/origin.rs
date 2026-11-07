//! Authentication origins retained by server-side sessions.

use crate::server::auth::token::{TokenChannels, TokenId, TokenMode, TokenState};
use std::sync::Arc;
/// Identifies how a session was authenticated.
#[derive(Clone)]
pub enum SessionOrigin {
    /// Session authenticated through SCRAM.
    Scram,
    /// Session authenticated with an API token.
    Token(TokenOrigin),
}
impl std::fmt::Debug for SessionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scram => f.write_str("Scram"),
            Self::Token(v) => f.debug_tuple("Token").field(v).finish(),
        }
    }
}
/// Token state retained by a token-authenticated session.
/// Fixed fields are copied from the immutable token state; revocation is observed with Acquire.
#[derive(Clone)]
pub struct TokenOrigin {
    /// Shared state containing the immutable token record and revocation flag.
    pub state: Arc<TokenState>,
}
impl std::fmt::Debug for TokenOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenOrigin")
            .field("token_id", &self.token_id())
            .finish()
    }
}
impl TokenOrigin {
    /// Returns the identifier of the originating token.
    pub fn token_id(&self) -> TokenId {
        TokenId(self.state.record().id)
    }
    /// Returns the token's mode, channels, and optional database scope.
    pub fn scope(&self) -> (TokenMode, TokenChannels, Option<&[String]>) {
        (
            self.state.record().mode,
            self.state.record().channels,
            self.state.record().databases.as_deref(),
        )
    }
    /// Reports whether the token is unrevoked and unexpired at `now_ns`.
    pub fn is_live(&self, now_ns: u64) -> bool {
        !self.state.is_revoked() && now_ns < self.state.record().expires_at_ns
    }
}
