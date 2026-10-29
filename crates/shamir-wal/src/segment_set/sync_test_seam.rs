//! Instance-local deterministic snapshot and actual sync-call handshakes.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct SyncTestSeam {
    pub(crate) sealed_error: AtomicUsize,
    pub(crate) pause_snapshot: AtomicBool,
    pub(crate) snapshot_paused: Notify,
    pub(crate) snapshot_resume: Notify,
    pub(crate) pause_sync: AtomicBool,
    pub(crate) sync_paused: Notify,
    pub(crate) sync_resume: Notify,
    pub(crate) in_flight: AtomicUsize,
    pub(crate) maximum: AtomicUsize,
    pub(crate) calls: AtomicUsize,
}

impl SyncTestSeam {
    pub(crate) async fn snapshot(&self) {
        if self.pause_snapshot.swap(false, Ordering::AcqRel) {
            self.snapshot_paused.notify_one();
            self.snapshot_resume.notified().await;
        }
    }
    pub(crate) async fn sync(&self) -> InFlight<'_> {
        let count = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        let guard = InFlight(self);
        self.maximum.fetch_max(count, Ordering::AcqRel);
        self.calls.fetch_add(1, Ordering::AcqRel);
        if self.pause_sync.swap(false, Ordering::AcqRel) {
            self.sync_paused.notify_one();
            self.sync_resume.notified().await;
        }
        guard
    }
}

pub(crate) struct InFlight<'a>(&'a SyncTestSeam);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub(crate) struct InjectedSealedError;

impl std::fmt::Display for InjectedSealedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("injected sealed I/O error")
    }
}

impl std::error::Error for InjectedSealedError {}

pub(crate) fn injected_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, InjectedSealedError)
}
