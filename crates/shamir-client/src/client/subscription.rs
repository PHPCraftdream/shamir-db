//! Push-frame demultiplexing for live subscriptions.

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::mpsc;
use tokio::sync::oneshot;

use shamir_connect::common::push_envelope::PushEnvelope;

use crate::error::CloseCause;

/// Per-subscription channel capacity. A stalled consumer can no longer
/// balloon client memory unboundedly — the reader task drops new pushes
/// (with a `tracing::warn!`) when this many envelopes are already queued
/// for the sub. The server-side `slow_consumer` push still fires first
/// at the bridge layer; this is the client-side backstop.
//
// TODO: lift to shamir-tunables when a client-tunables module is added.
pub(crate) const CLIENT_SUB_CHANNEL_CAP: usize = 256;

/// Sender half for routing push frames to a subscription handle.
pub type PushSender = mpsc::Sender<PushEnvelope>;

/// Receiver half yielding push frames to the subscription consumer.
pub type PushReceiver = mpsc::Receiver<PushEnvelope>;

pub(crate) const EARLY_BUFFER_CAP: usize = 256;

/// Subscription registry (live senders + early buffer + close cause). See
/// [`crate::client::transport::SubsInner`] for the contention model.
pub(crate) type SubsMap = Arc<StdMutex<crate::client::transport::SubsInner>>;

/// Handle to a live subscription stream. Yields push envelopes as they
/// arrive; after the connection terminates, queued items drain and the
/// stream ends (`next()` → `None`), with the terminal cause available via
/// [`Self::termination`].
pub struct SubscriptionHandle {
    /// Server-assigned subscription id.
    pub sub_id: u64,
    rx: PushReceiver,
    registry: SubsMap,
    /// Typed termination channel: resolved by `Lifecycle::shutdown` (or
    /// pre-resolved when the handle was created on a closed client).
    term_rx: Option<oneshot::Receiver<CloseCause>>,
    /// Cached cause once observed.
    terminal: Option<CloseCause>,
}

impl SubscriptionHandle {
    /// Create a new handle (crate-internal).
    pub(crate) fn new(
        sub_id: u64,
        rx: PushReceiver,
        registry: SubsMap,
        termination: oneshot::Receiver<CloseCause>,
    ) -> Self {
        Self {
            sub_id,
            rx,
            registry,
            term_rx: Some(termination),
            terminal: None,
        }
    }

    /// Receive the next push envelope, or `None` once the subscription
    /// channel ended (connection terminal transition).
    pub async fn next(&mut self) -> Option<PushEnvelope> {
        self.rx.recv().await
    }

    /// The terminal close cause, `Some` once the subscription has ended.
    /// Synchronous: reads the cached cause or non-blockingly polls the
    /// termination channel (`None` = not yet terminated; once `next()`
    /// returned `None` after a shutdown the cause is always present).
    pub fn termination(&mut self) -> Option<CloseCause> {
        if let Some(cause) = &self.terminal {
            return Some(cause.clone());
        }
        let cause = match self.term_rx.as_mut()?.try_recv() {
            // Channel is single-shot; stop tracking it once drained.
            Ok(cause) => {
                self.term_rx = None;
                Some(cause)
            }
            Err(_) => None,
        };
        self.terminal = cause.clone();
        cause
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        // Remove from registry so no more pushes are routed here and no
        // terminal send is attempted on a dead handle.
        let mut subs = self.registry.lock().unwrap_or_else(|p| p.into_inner());
        subs.live.remove(&self.sub_id);
        subs.terminal.remove(&self.sub_id);
    }
}
