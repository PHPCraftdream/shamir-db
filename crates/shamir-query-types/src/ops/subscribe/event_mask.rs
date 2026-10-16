use serde::{Deserialize, Serialize};

/// Which change operations the subscription listens to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventMask {
    #[default]
    All,
    Put,
    Delete,
}
