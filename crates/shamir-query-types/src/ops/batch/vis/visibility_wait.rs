use serde::{Deserialize, Serialize};

/// Commit acknowledgement policy. Zero milliseconds means an immediate predicate
/// check, never an infinite wait. VIS-1 carries this policy without enabling it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum VisibilityWait {
    #[default]
    Strict,
    Bounded {
        timeout_ms: u64,
    },
}

impl VisibilityWait {
    /// Validate an override against an externally supplied policy; never clamp.
    pub fn validate(self, max_wait_ms: u64, allow_strict: bool) -> Result<(), &'static str> {
        match self {
            Self::Strict if !allow_strict => Err("strict visibility wait is forbidden"),
            Self::Bounded { timeout_ms } if timeout_ms > max_wait_ms => {
                Err("visibility wait exceeds maximum")
            }
            Self::Strict | Self::Bounded { .. } => Ok(()),
        }
    }
}
