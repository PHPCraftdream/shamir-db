use serde::{Deserialize, Serialize};

use super::{ApiTokenChannels, ApiTokenMode};

/// Public API-token listing row; it contains no secret, MAC, or hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTokenView {
    /// Token identifier, base64url-encoded from 16 bytes.
    pub id: String,
    /// Human-readable token label.
    pub label: String,
    /// Owner username.
    pub owner: String,
    /// Granted access mode.
    pub mode: ApiTokenMode,
    /// Allowed request channels.
    pub channels: ApiTokenChannels,
    /// Database allowlist; `None` means all accessible databases.
    pub databases: Option<Vec<String>>,
    /// Creation time in Unix nanoseconds.
    pub created_at_ns: u64,
    /// Expiration time in Unix nanoseconds.
    pub expires_at_ns: u64,
    /// Revocation time in Unix nanoseconds, if revoked.
    pub revoked_at_ns: Option<u64>,
}
