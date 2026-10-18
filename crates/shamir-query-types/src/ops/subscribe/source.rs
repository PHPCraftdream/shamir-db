use serde::{Deserialize, Serialize};

use crate::filter::Filter;
use crate::TableRef;

use super::event_mask::EventMask;

/// A single table source for a subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionSource {
    pub table: TableRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    #[serde(default)]
    pub events: EventMask,
}
