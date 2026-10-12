use serde::{Deserialize, Serialize};

use super::VisibilityPendingReason;

/// Contiguous settled-prefix observation, independent of durability and
/// materialization. `blocking_version` is exactly `visible_through + 1`, the
/// first position outside the sampled prefix, NOT evidence of a wedged owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum VisibilityState {
    Visible,
    Pending {
        visible_through: u64,
        blocking_version: u64,
        reason: VisibilityPendingReason,
    },
}

impl VisibilityState {
    pub fn validate(self) -> Result<(), &'static str> {
        match self {
            Self::Visible => Ok(()),
            Self::Pending {
                visible_through,
                blocking_version,
                ..
            } => {
                if visible_through.checked_add(1) == Some(blocking_version) {
                    Ok(())
                } else {
                    Err("blocking_version must be visible_through + 1")
                }
            }
        }
    }
}
