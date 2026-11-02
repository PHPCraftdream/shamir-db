use shamir_connect::server::token::{TokenError, TokenId, TokenSecret};

/// Callable used to generate token ID and secret pairs.
pub(crate) type TokenGenerator =
    dyn Fn() -> Result<(TokenId, TokenSecret), TokenError> + Send + Sync;
