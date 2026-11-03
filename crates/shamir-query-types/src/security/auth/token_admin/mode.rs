use serde::{Deserialize, Serialize};

/// Access mode granted by an API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiTokenMode {
    /// Read operations only.
    ReadOnly,
    /// Read and write operations.
    ReadWrite,
}
