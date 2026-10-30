use super::{Action, Actor};

/// Access denied (constructed by the real policy check in P4).
#[derive(Debug, thiserror::Error)]
#[error("access denied: {actor} cannot {action} on {}", super::escape_resource_diagnostic(.path.as_str()))]
pub struct AccessError {
    pub actor: Actor,
    /// The rendered resource path (kept as a `String`, not the full
    /// `ResourcePath` enum, so the error stays small — `Result<_, AccessError>`
    /// is on the hot path and `clippy::result_large_err` would fire otherwise.
    pub path: String,
    pub action: Action,
}
