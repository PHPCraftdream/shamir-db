use serde::{Deserialize, Serialize};

use super::deliver_mode::DeliverMode;
use super::source::SubscriptionSource;

/// Subscribe to table change events.
///
/// Wire discriminator key: `"subscribe"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribeOp {
    pub subscribe: Vec<SubscriptionSource>,
    #[serde(default)]
    pub deliver: DeliverMode,
    #[serde(default)]
    pub initial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<u64>,
}
