//! Push-frame demultiplexing for live subscriptions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::mpsc;

use shamir_connect::common::push_envelope::PushEnvelope;

/// Sender half for routing push frames to a subscription handle.
pub type PushSender = mpsc::UnboundedSender<PushEnvelope>;

/// Receiver half yielding push frames to the subscription consumer.
pub type PushReceiver = mpsc::UnboundedReceiver<PushEnvelope>;

/// Registry of active subscription channels, keyed by `sub_id`.
/// Push frames are routed here by the reader task.
pub(crate) type SubscriptionMap = Arc<StdMutex<HashMap<u64, PushSender>>>;

pub(crate) const EARLY_BUFFER_CAP: usize = 256;

/// Bounded per-sub early buffer for pushes arriving before `subscribe_push`.
pub(crate) type EarlyBuffer = Arc<StdMutex<HashMap<u64, Vec<PushEnvelope>>>>;

/// Handle to a live subscription stream. Yields push envelopes as they arrive.
pub struct SubscriptionHandle {
    /// Server-assigned subscription id.
    pub sub_id: u64,
    rx: PushReceiver,
    registry: SubscriptionMap,
}

impl SubscriptionHandle {
    /// Create a new handle (crate-internal).
    pub(crate) fn new(sub_id: u64, rx: PushReceiver, registry: SubscriptionMap) -> Self {
        Self {
            sub_id,
            rx,
            registry,
        }
    }

    /// Receive the next push envelope, or `None` if the subscription was closed.
    pub async fn next(&mut self) -> Option<PushEnvelope> {
        self.rx.recv().await
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        // Remove from registry so no more pushes are routed here.
        let mut map = self.registry.lock().unwrap_or_else(|p| p.into_inner());
        map.remove(&self.sub_id);
    }
}
