use shamir_connect::server::token::{TokenChannels, TokenMode};

/// Validated-on-create request for a new token.
#[derive(Debug, Clone)]
pub struct NewToken {
    /// Owning user identifier.
    pub owner_user_id: [u8; 16],
    /// Roles recorded on the token.
    pub owner_roles: Vec<String>,
    /// User-selected label, unique among the owner's active tokens.
    pub label: String,
    /// Requested lifetime in seconds; the configured default when absent.
    pub ttl_secs: Option<u64>,
    /// Granted access mode.
    pub mode: TokenMode,
    /// Allowed endpoint channels.
    pub channels: TokenChannels,
    /// Optional database allowlist.
    pub databases: Option<Vec<String>>,
}
