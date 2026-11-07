//! Typed payload returned once when an API token is created.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

use super::{ApiTokenChannels, ApiTokenMode};
use crate::auth::SecretString;

/// Successful API-token creation payload; its secret is returned exactly once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedApiToken {
    /// Token ID, base64url-encoded from 16 bytes.
    pub id: String,
    /// Secret bearer credential, returned exactly once and redacted by `Debug`.
    pub token: SecretString,
    /// Token label.
    pub label: String,
    /// Owner username.
    pub owner: String,
    /// Scope mode.
    pub mode: ApiTokenMode,
    /// Allowed channels.
    pub channels: ApiTokenChannels,
    /// Database allowlist, or `None` for all accessible databases.
    pub databases: Option<Vec<String>>,
    /// Creation time in unix nanoseconds.
    pub created_at_ns: u64,
    /// Expiration time in unix nanoseconds.
    pub expires_at_ns: u64,
}
