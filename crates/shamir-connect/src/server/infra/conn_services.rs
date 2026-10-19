use std::sync::Arc;

/// Marker for a rejected push (channel full / closed).
#[derive(Debug)]
pub struct PushRejected;

/// Trait for sending unsolicited (push) frames to the client.
pub trait PushSink: Send + Sync {
    /// Attempt to enqueue a push frame; fails if the channel is full or closed.
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected>;
}

/// Per-connection services visible to the request handler.
///
/// Passed into `RequestHandler::handle` so the handler can discover
/// the connection it is running on (e.g. to activate subscriptions).
pub struct ConnectionServices {
    /// Unique identifier for this connection (assigned by the listener).
    pub conn_id: u64,
    /// Optional push channel for server-initiated frames (subscriptions).
    pub push: Option<Arc<dyn PushSink>>,
    /// Opaque extension point for server-layer per-connection state
    /// (e.g. `SubscriptionRegistry`). The handler downcasts via `Any`.
    pub extensions: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl ConnectionServices {
    /// Convenience: no push channel available (default until subscriptions land).
    pub fn without_push(conn_id: u64) -> Self {
        Self {
            conn_id,
            push: None,
            extensions: None,
        }
    }
}
