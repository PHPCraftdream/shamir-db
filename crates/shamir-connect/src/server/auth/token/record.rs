use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenMode {
    #[serde(rename = "read_only")]
    ReadOnly,
    #[serde(rename = "read_write")]
    ReadWrite,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenChannels {
    #[serde(rename = "rest")]
    Rest,
    #[serde(rename = "ws")]
    Ws,
    #[serde(rename = "both")]
    Both,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Rest,
    Ws,
}
impl TokenChannels {
    pub fn allows(self, endpoint: Endpoint) -> bool {
        matches!(
            (self, endpoint),
            (Self::Both, _) | (Self::Rest, Endpoint::Rest) | (Self::Ws, Endpoint::Ws)
        )
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRecord {
    #[serde(with = "serde_bytes")]
    pub id: [u8; 16],
    #[serde(with = "serde_bytes")]
    pub owner_user_id: [u8; 16],
    pub label: String,
    #[serde(with = "serde_bytes")]
    pub mac: [u8; 32],
    pub key_id: u8,
    pub format_version: u8,
    pub created_at_ns: u64,
    pub expires_at_ns: u64,
    pub mode: TokenMode,
    pub channels: TokenChannels,
    pub databases: Option<Vec<String>>,
    pub roles: Vec<String>,
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
