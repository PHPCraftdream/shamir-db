use crate::server::auth::token::{TokenChannels, TokenId, TokenMode, TokenState};
use std::sync::Arc;
#[derive(Clone)]
pub enum SessionOrigin {
    Scram,
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
/// Fixed fields are copied from the immutable token state; revocation is observed with Acquire.
#[derive(Clone)]
pub struct TokenOrigin {
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
    pub fn token_id(&self) -> TokenId {
        TokenId(self.state.record().id)
    }
    pub fn scope(&self) -> (TokenMode, TokenChannels, Option<&[String]>) {
        (
            self.state.record().mode,
            self.state.record().channels,
            self.state.record().databases.as_deref(),
        )
    }
    pub fn is_live(&self, now_ns: u64) -> bool {
        !self.state.is_revoked() && now_ns < self.state.record().expires_at_ns
    }
}
