use serde::{Deserialize, Serialize};

/// Cancel an active subscription by its server-assigned id.
///
/// Wire discriminator key: `"unsubscribe"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnsubscribeOp {
    pub unsubscribe: u64,
}
