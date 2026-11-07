//! Revocable changefeed receiver implementation.
use super::registry::ChangelogReceiverRegistry;
use shamir_engine::ChangelogEvent;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Notify};

const CLOSED: usize = 1usize << (usize::BITS - 1);
const ACTIVE: usize = CLOSED - 1;
#[derive(Clone)]
pub struct ChangelogDeliveryGate(pub(super) Arc<GateState>);
pub(super) struct GateState {
    state: AtomicUsize,
    notify: Notify,
}
struct Lease<'a>(&'a GateState);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let previous = self.0.state.fetch_sub(1, Ordering::SeqCst);
        if previous & ACTIVE == 1 {
            self.0.notify.notify_waiters();
        }
    }
}
impl ChangelogDeliveryGate {
    pub(crate) fn new() -> Self {
        Self(Arc::new(GateState {
            state: AtomicUsize::new(0),
            notify: Notify::new(),
        }))
    }
    pub fn try_with_delivery<T>(&self, accept: impl FnOnce() -> T) -> Option<T> {
        let mut state = self.0.state.load(Ordering::SeqCst);
        loop {
            if state & CLOSED != 0 || state & ACTIVE == ACTIVE {
                return None;
            }
            match self.0.state.compare_exchange_weak(
                state,
                state + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }
        let lease = Lease(&self.0);
        let value = accept();
        drop(lease);
        Some(value)
    }
    pub fn closed_now(&self) -> bool {
        self.0.state.load(Ordering::SeqCst) & CLOSED != 0
    }
    pub async fn closed(&self) {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed_now() {
                return;
            }
            notified.await;
        }
    }
    pub(crate) fn close(&self) {
        self.0.state.fetch_or(CLOSED, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }
    pub(crate) async fn wait_idle(&self) {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.0.state.load(Ordering::SeqCst) & ACTIVE == 0 {
                return;
            }
            notified.await;
        }
    }
}
pub struct ChangelogReceiver {
    receiver: broadcast::Receiver<Arc<ChangelogEvent>>,
    gate: ChangelogDeliveryGate,
    registry: ChangelogReceiverRegistry,
    db: String,
    repo: String,
}
impl ChangelogReceiver {
    pub(crate) fn new(
        receiver: broadcast::Receiver<Arc<ChangelogEvent>>,
        registry: ChangelogReceiverRegistry,
        db: String,
        repo: String,
    ) -> Self {
        Self {
            receiver,
            gate: ChangelogDeliveryGate::new(),
            registry,
            db,
            repo,
        }
    }
    pub fn delivery_gate(&self) -> ChangelogDeliveryGate {
        self.gate.clone()
    }
    pub async fn recv(&mut self) -> Result<Arc<ChangelogEvent>, broadcast::error::RecvError> {
        if self.gate.closed_now() {
            return Err(broadcast::error::RecvError::Closed);
        }
        let event = tokio::select! { biased; () = self.gate.closed() => return Err(broadcast::error::RecvError::Closed), result = self.receiver.recv() => result? };
        self.gate
            .try_with_delivery(|| event)
            .ok_or(broadcast::error::RecvError::Closed)
    }
    pub fn try_recv(&mut self) -> Result<Arc<ChangelogEvent>, broadcast::error::TryRecvError> {
        if self.gate.closed_now() {
            return Err(broadcast::error::TryRecvError::Closed);
        }
        let event = match self.receiver.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                if self.gate.closed_now() {
                    return Err(broadcast::error::TryRecvError::Closed);
                }
                return Err(broadcast::error::TryRecvError::Lagged(skipped));
            }
            result => result?,
        };
        self.gate
            .try_with_delivery(|| event)
            .ok_or(broadcast::error::TryRecvError::Closed)
    }
}
impl Drop for ChangelogReceiver {
    fn drop(&mut self) {
        self.gate.close();
        self.registry.unregister(&self.db, &self.repo, &self.gate);
    }
}
