use shamir_connect::server::token::TokenId;

/// Cancels live sessions and work bound to a revoked token.
pub trait SessionCanceller: Send + Sync {
    /// Cancel everything bound to this token.
    fn cancel_token(&self, id: &TokenId);
}

/// Canceller used until a transport installs a real one.
pub(super) struct NoopCanceller;

impl SessionCanceller for NoopCanceller {
    fn cancel_token(&self, _: &TokenId) {}
}
