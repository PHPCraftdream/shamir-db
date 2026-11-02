use super::token_view::TokenView;
use zeroize::Zeroizing;

/// One-time token creation result; Debug intentionally excludes its bearer token.
pub struct CreatedToken {
    /// Non-secret token view.
    pub record_view: TokenView,
    /// Bearer token, returned once to the caller and zeroized on drop.
    pub token: Zeroizing<String>,
}

impl std::fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedToken")
            .field("record_view", &self.record_view)
            .field("token", &"<REDACTED>")
            .finish()
    }
}
