use shamir_connect::server::token::{TokenChannels, TokenMode, TokenRecord};

/// Secret-free public projection of a token record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenView {
    /// Token identifier.
    pub id: [u8; 16],
    /// Owning user identifier.
    pub owner_user_id: [u8; 16],
    /// User-selected label.
    pub label: String,
    /// Creation timestamp in Unix nanoseconds.
    pub created_at_ns: u64,
    /// Expiration timestamp in Unix nanoseconds.
    pub expires_at_ns: u64,
    /// Granted access mode.
    pub mode: TokenMode,
    /// Allowed endpoint channels.
    pub channels: TokenChannels,
    /// Optional database allowlist.
    pub databases: Option<Vec<String>>,
    /// Revocation timestamp, if revoked.
    pub revoked_at_ns: Option<u64>,
}

impl From<&TokenRecord> for TokenView {
    fn from(record: &TokenRecord) -> Self {
        Self {
            id: record.id,
            owner_user_id: record.owner_user_id,
            label: record.label.clone(),
            created_at_ns: record.created_at_ns,
            expires_at_ns: record.expires_at_ns,
            mode: record.mode,
            channels: record.channels,
            databases: record.databases.clone(),
            revoked_at_ns: record.revoked_at_ns,
        }
    }
}
