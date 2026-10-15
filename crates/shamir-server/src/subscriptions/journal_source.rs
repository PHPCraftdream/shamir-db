use std::future::Future;
use std::sync::Arc;

use futures_util::StreamExt;
use shamir_db::engine::BoundedJournalRead;
use shamir_db::shamir_db::ChangelogDeliveryGate;
use shamir_tx::ChangelogEvent;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

/// Wait until any attached repo gate is revoked.
pub(super) async fn wait_for_closed(gates: &[ChangelogDeliveryGate]) {
    let mut waits = futures_util::stream::FuturesUnordered::new();
    for gate in gates {
        waits.push(gate.closed());
    }
    let _ = waits.next().await;
}

/// Receive-only view shared by raw test feeds and revocable facade feeds.
/// Both implementations preserve Tokio broadcast lag/closed semantics.
pub(super) trait JournalReceiver: Send {
    fn recv(&mut self) -> impl Future<Output = Result<Arc<ChangelogEvent>, RecvError>> + Send;
    fn try_recv(&mut self) -> Result<Arc<ChangelogEvent>, TryRecvError>;
}

impl JournalReceiver for tokio::sync::broadcast::Receiver<Arc<ChangelogEvent>> {
    async fn recv(&mut self) -> Result<Arc<ChangelogEvent>, RecvError> {
        tokio::sync::broadcast::Receiver::recv(self).await
    }

    fn try_recv(&mut self) -> Result<Arc<ChangelogEvent>, TryRecvError> {
        tokio::sync::broadcast::Receiver::try_recv(self)
    }
}

impl JournalReceiver for shamir_db::shamir_db::ChangelogReceiver {
    async fn recv(&mut self) -> Result<Arc<ChangelogEvent>, RecvError> {
        shamir_db::shamir_db::ChangelogReceiver::recv(self).await
    }

    fn try_recv(&mut self) -> Result<Arc<ChangelogEvent>, TryRecvError> {
        shamir_db::shamir_db::ChangelogReceiver::try_recv(self)
    }
}

/// Internal RPITIT + Send seam: engine hooks are crate-test-only. No dyn API.
pub(super) trait JournalSource: Sync {
    /// Observation seam for deterministic tests; production has no idle work.
    fn on_idle(&self) {}

    /// Checked immediately after each awaited page read and before its contents
    /// are examined. Sources without revocation retain the historical behavior.
    fn quarantined(&self, _repo: &str) -> bool {
        false
    }

    fn read(
        &self,
        repo: &str,
        from: u64,
    ) -> impl Future<Output = Result<BoundedJournalRead, String>> + Send;

    /// cancel-safe: yes — completion observation only, no payload consumption.
    fn progress(&self, repo: &str, target: u64) -> impl Future<Output = Result<(), String>> + Send;
}
