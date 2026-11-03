use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::token::{OwnerFloors, TokenError, TokenId, TokenSecret};
use shamir_connect::server::TokenSessionStore;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Callable used to generate token ID and secret pairs.
pub(crate) type TokenGenerator =
    dyn Fn() -> Result<(TokenId, TokenSecret), TokenError> + Send + Sync;

/// Dependencies and settings for opening a `TokenService`.
pub struct TokenServiceOptions {
    /// Directory holding the durable token store.
    pub data_dir: std::path::PathBuf,
    /// Persisted MAC key.
    pub token_key: Zeroizing<[u8; 32]>,
    /// Per-owner revocation floors.
    pub owners: Arc<dyn OwnerFloors + Send + Sync>,
    /// Sessions opened by tokens.
    pub sessions: Arc<TokenSessionStore>,
    /// Shared audit chain.
    pub audit: Arc<AuditChainWriter>,
    /// Token limits and lifetimes.
    pub config: crate::config::TokensConfig,
    /// Reject mutations on this node.
    pub read_only_node: bool,
    /// Wall clock in Unix nanoseconds; `None` when unreadable.
    pub clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
}
