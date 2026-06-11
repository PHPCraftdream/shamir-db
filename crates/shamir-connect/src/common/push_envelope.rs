//! Server-initiated push envelope for live subscription notifications.

use serde::{Deserialize, Serialize};

/// Kind of server-initiated push frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    /// A normal change event carrying record data.
    Event,
    /// A gap in the event stream — some events were missed.
    Gap,
    /// The subscriber is too slow; the server dropped events.
    SlowConsumer,
    /// Initial snapshot delivery is complete; live events follow.
    Ready,
    /// The subscription has been closed by the server.
    Closed,
}

/// Server-initiated push envelope — distinguished from ResponseEnvelope
/// by the presence of a `"push"` key (vs `"rid"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushEnvelope {
    /// The kind of push notification.
    pub push: PushKind,
    /// Server-assigned subscription id this push belongs to.
    pub sub: u64,
    /// Monotonic sequence number within the subscription.
    pub seq: u64,
    /// Optional payload (MessagePack-encoded records, keys, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    /// For `Gap` pushes — the version at which the gap starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_at: Option<u64>,
}
