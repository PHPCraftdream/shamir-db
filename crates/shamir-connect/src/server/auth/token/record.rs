use serde::{Deserialize, Serialize};
/// Access mode granted by a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenMode {
    #[serde(rename = "read_only")]
    /// Allows read operations only.
    ReadOnly,
    #[serde(rename = "read_write")]
    /// Allows read and write operations.
    ReadWrite,
}
/// Network channels on which a token is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenChannels {
    #[serde(rename = "rest")]
    /// Accepts REST requests.
    Rest,
    #[serde(rename = "ws")]
    /// Accepts WebSocket requests.
    Ws,
    #[serde(rename = "both")]
    /// Accepts REST and WebSocket requests.
    Both,
}
/// Request endpoint presented to token channel policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// REST request endpoint.
    Rest,
    /// WebSocket request endpoint.
    Ws,
}
impl TokenChannels {
    /// Returns whether this channel policy admits the endpoint.
    pub fn allows(self, endpoint: Endpoint) -> bool {
        matches!(
            (self, endpoint),
            (Self::Both, _) | (Self::Rest, Endpoint::Rest) | (Self::Ws, Endpoint::Ws)
        )
    }
}
/// Persistable token metadata and authentication MAC.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRecord {
    #[serde(with = "serde_bytes")]
    /// Token identifier bytes.
    pub id: [u8; 16],
    #[serde(with = "serde_bytes")]
    /// Owning user identifier bytes.
    pub owner_user_id: [u8; 16],
    /// User-visible label, at most 64 bytes.
    pub label: String,
    #[serde(with = "serde_bytes")]
    /// Authentication MAC; secret-derived bytes are redacted in Debug.
    pub mac: [u8; 32],
    /// Identifier of the MAC key used.
    pub key_id: u8,
    /// Token format version used for authentication.
    pub format_version: u8,
    /// Creation time in Unix nanoseconds.
    pub created_at_ns: u64,
    /// Expiration time in Unix nanoseconds.
    pub expires_at_ns: u64,
    /// Granted read/write access mode.
    pub mode: TokenMode,
    /// Allowed request channels.
    pub channels: TokenChannels,
    /// Optional database allowlist; each name is at most 128 bytes.
    pub databases: Option<Vec<String>>,
    /// Snapshot of owner roles at token creation.
    pub roles: Vec<String>,
    /// Revocation time in Unix nanoseconds, if revoked.
    pub revoked_at_ns: Option<u64>,
}
impl std::fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRecord")
            .field("id", &"<id>")
            .field("owner_user_id", &"<owner>")
            .field("label", &self.label)
            .field("mac", &"<REDACTED>")
            .field("key_id", &self.key_id)
            .field("format_version", &self.format_version)
            .field("created_at_ns", &self.created_at_ns)
            .field("expires_at_ns", &self.expires_at_ns)
            .field("mode", &self.mode)
            .field("channels", &self.channels)
            .field("databases", &self.databases)
            .field("roles", &self.roles)
            .field("revoked_at_ns", &self.revoked_at_ns)
            .finish()
    }
}
impl TokenRecord {
    /// Validates structural limits and timestamp ordering.
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        if self.expires_at_ns <= self.created_at_ns {
            return Err("expiry must follow creation");
        }
        if self.label.is_empty() || self.label.len() > 64 {
            return Err("invalid label");
        }
        if self.databases.as_ref().is_some_and(|items| {
            items.len() > 64 || items.iter().any(|name| name.is_empty() || name.len() > 128)
        }) {
            return Err("invalid databases");
        }
        if self.roles.len() > 64 {
            return Err("too many roles");
        }
        Ok(())
    }
}
