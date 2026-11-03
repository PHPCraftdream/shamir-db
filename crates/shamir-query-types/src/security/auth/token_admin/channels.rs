use serde::{Deserialize, Serialize};

/// Allowed request channels for an API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiTokenChannels {
    /// REST only.
    Rest,
    /// WebSocket only.
    Ws,
    /// REST and WebSocket.
    Both,
}
