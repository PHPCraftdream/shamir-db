use serde::{Deserialize, Serialize};

/// Cancel an active subscription by its server-assigned id.
///
/// Wire discriminator key: `"unsubscribe"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsubscribeOp {
    pub unsubscribe: u64,
}
