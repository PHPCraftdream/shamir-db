use serde::{Deserialize, Serialize};

/// Authorization mode requested for the token minted by a password login.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginMode {
    ReadOnly,
    ReadWrite,
}

/// Channels the minted token may be used on.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginChannels {
    Rest,
    Ws,
    Both,
}

/// Scope of the token minted by a password login.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginScope {
    pub label: String,
    pub mode: LoginMode,
    pub channels: LoginChannels,
    /// `None` means every database; otherwise a non-empty allowlist.
    pub databases: Option<Vec<String>>,
}

impl Default for LoginScope {
    fn default() -> Self {
        Self {
            label: "http-login".to_owned(),
            mode: LoginMode::ReadOnly,
            channels: LoginChannels::Rest,
            databases: None,
        }
    }
}

impl LoginScope {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_mode(mut self, mode: LoginMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_channels(mut self, channels: LoginChannels) -> Self {
        self.channels = channels;
        self
    }

    pub fn with_databases(mut self, databases: Option<Vec<String>>) -> Self {
        self.databases = databases;
        self
    }
}
