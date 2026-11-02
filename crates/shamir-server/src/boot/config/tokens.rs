use serde::Deserialize;

/// Token-related configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokensConfig {
    /// Default token lifetime in days.
    #[serde(default = "default_ttl_days")]
    pub default_ttl_days: u64,
    /// Maximum permitted token lifetime in days.
    #[serde(default = "max_ttl_days")]
    pub max_ttl_days: u64,
    /// Maximum active tokens per owner.
    #[serde(default = "max_per_user")]
    pub max_per_user: usize,
    /// Whether ordinary users may create their own tokens.
    #[serde(default = "allow_self_service")]
    pub allow_self_service: bool,
    /// Maximum token-session lifetime in hours.
    #[serde(default = "session_max_age_hours")]
    pub session_max_age_hours: u64,
    /// Maximum concurrent sockets per token.
    #[serde(default = "max_sockets_per_token")]
    pub max_sockets_per_token: usize,
    /// Maximum concurrent sockets per owner.
    #[serde(default = "max_sockets_per_owner")]
    pub max_sockets_per_owner: usize,
}

impl Default for TokensConfig {
    fn default() -> Self {
        Self {
            default_ttl_days: default_ttl_days(),
            max_ttl_days: max_ttl_days(),
            max_per_user: max_per_user(),
            allow_self_service: allow_self_service(),
            session_max_age_hours: session_max_age_hours(),
            max_sockets_per_token: max_sockets_per_token(),
            max_sockets_per_owner: max_sockets_per_owner(),
        }
    }
}

impl TokensConfig {
    /// Validate token lifetime and concurrent-use caps.
    pub fn validate(&self) -> Result<(), String> {
        if self.default_ttl_days == 0 || self.max_ttl_days == 0 {
            return Err("default_ttl_days and max_ttl_days must be positive".into());
        }
        if self.default_ttl_days > self.max_ttl_days {
            return Err("default_ttl_days must be <= max_ttl_days".into());
        }
        if self.max_per_user == 0
            || self.session_max_age_hours == 0
            || self.max_sockets_per_token == 0
            || self.max_sockets_per_owner == 0
            || self.max_sockets_per_owner < self.max_sockets_per_token
        {
            return Err(
                "token count and session/socket caps must be positive and owner socket cap must cover token cap"
                    .into(),
            );
        }
        if self.max_ttl_days > 365
            || self.max_per_user > 1_000
            || self.session_max_age_hours > 24
            || self.max_sockets_per_token > 1_024
            || self.max_sockets_per_owner > 4_096
        {
            return Err("token configuration exceeds sane caps".into());
        }
        Ok(())
    }
}

fn default_ttl_days() -> u64 {
    30
}
fn max_ttl_days() -> u64 {
    365
}
fn max_per_user() -> usize {
    20
}
fn allow_self_service() -> bool {
    true
}
fn session_max_age_hours() -> u64 {
    24
}
fn max_sockets_per_token() -> usize {
    8
}
fn max_sockets_per_owner() -> usize {
    32
}
